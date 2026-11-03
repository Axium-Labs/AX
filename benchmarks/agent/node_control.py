"""Explicit bounded control of dedicated benchmark nodes; never discover/kill user processes.

Commands are argv arrays. Remote commands use SSH BatchMode and POSIX argument
quoting. Every mutating command must carry the generated run identity. Operators
supply worker-specific start/stop/disconnect/reconnect scripts; no guessed PIDs,
firewall rules, cloud provisioning or installed-program operations are performed.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import time

from execution import stop_tree

MUTATIONS = {"prepare", "start", "stop", "crash", "disconnect", "reconnect", "cleanup"}
IDENTITY = re.compile(r"^[A-Za-z0-9_-]{1,96}$")


def validate_node(node):
    if not IDENTITY.fullmatch(node.get("id", "")):
        raise ValueError("node id must be a bounded safe identifier")
    if node.get("transport", "local") not in {"local", "ssh"}:
        raise ValueError("node transport must be local or ssh; simulated nodes are not supported")
    if node.get("transport") == "ssh":
        host = node.get("ssh_host", "")
        if not host or host.startswith("-") or any(c.isspace() for c in host):
            raise ValueError("invalid SSH destination")
    for name, argv in node.get("commands", {}).items():
        if not isinstance(argv, list) or not argv or not all(isinstance(a, str) and a for a in argv):
            raise ValueError(f"{node['id']}: {name} must be a nonempty argv array")
        if name in MUTATIONS and not any("{run_id}" in a for a in argv):
            raise ValueError(f"{node['id']}: mutating {name} must address the benchmark run_id")
    if node.get("commands") and not node.get("dedicated_benchmark_environment"):
        raise ValueError("node commands require a dedicated benchmark environment")
    if any(k in node.get("commands", {}) for k in ("prepare", "start")) and "cleanup" not in node.get("commands", {}):
        raise ValueError("managed node prepare/start requires an explicit run-scoped cleanup command")


def render_command(node, name, context):
    validate_node(node)
    if name not in node.get("commands", {}):
        raise ValueError(f"{node['id']}: missing explicit {name} command")
    argv = []
    for value in node["commands"][name]:
        for key, replacement in context.items():
            value = value.replace("{"+key+"}", str(replacement))
        argv.append(value)
    if node.get("transport", "local") == "ssh":
        # SSH invokes a remote shell. Quote each already-separated argument once.
        argv = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "--", node["ssh_host"], shlex.join(argv)]
    return argv


def node_command(node, name, context, output, timeout=20, max_output_bytes=4_194_304):
    if timeout <= 0:
        raise TimeoutError("node command has no remaining deadline")
    argv = render_command(node, name, context)
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    stdout_path, stderr_path = output/"stdout.txt", output/"stderr.txt"
    started = time.monotonic()
    reason = None
    with stdout_path.open("wb") as out, stderr_path.open("wb") as err:
        proc = subprocess.Popen(argv, cwd=node.get("local_cwd") if node.get("transport", "local") == "local" else None,
            stdin=subprocess.DEVNULL, stdout=out, stderr=err, env=os.environ.copy(), start_new_session=os.name != "nt")
        try:
            while proc.poll() is None:
                if time.monotonic()-started >= timeout:
                    reason = "timeout"
                    break
                if stdout_path.stat().st_size+stderr_path.stat().st_size > max_output_bytes:
                    reason = "output_limit"
                    break
                time.sleep(.02)
        finally:
            stop_tree(proc)
    result = dict(command_sha256=hashlib.sha256(json.dumps(argv).encode()).hexdigest(),
        exit_code=proc.returncode, reason=reason, wall_seconds=time.monotonic()-started,
        stdout=str(stdout_path), stderr=str(stderr_path))
    if stdout_path.stat().st_size+stderr_path.stat().st_size > max_output_bytes:
        result["reason"] = reason = "output_limit"
    if proc.returncode == 0 and reason is None:
        try:
            result["json"] = json.loads(stdout_path.read_text(encoding="utf-8"))
        except ValueError:
            pass
    (output/"command.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    if proc.returncode or reason:
        raise RuntimeError(f"node command {node['id']}/{name} failed ({reason or proc.returncode}); see retained command logs")
    return result


def resources_summary(samples, expected_nodes):
    """Delta cumulative counters by node/process identity, never sum snapshots."""
    result = {"cpu_seconds": None, "peak_rss_bytes": None, "network_bytes": None,
              "input_tokens": None, "output_tokens": None, "api_requests": None, "cost_microusd": None,
              "node_count": len(expected_nodes), "sample_count": len(samples), "resets": 0,
              "usage_estimated": None, "cost_estimated": None}
    if not expected_nodes:
        return result
    groups = {}
    for sample in samples:
        if sample.get("node_id") in expected_nodes:
            groups.setdefault(sample["node_id"], []).append(sample)
    for field in ("cpu_seconds",):
        totals = []
        for node in expected_nodes:
            rows = groups.get(node, [])
            if len(rows) < 2 or any(r.get(field) is None for r in rows):
                break
            total = 0
            previous = rows[0]
            for current in rows[1:]:
                same_process = current.get("process_identity") == previous.get("process_identity")
                if same_process and current[field] >= previous[field]:
                    total += current[field]-previous[field]
                else:
                    # A new process has its own lifetime counters. Attribute its known
                    # current usage, retaining the reset count and exact lower boundary.
                    total += current[field]
                    result["resets"] += 1
                previous = current
            totals.append(total)
        else:
            result[field] = sum(totals)
    network_groups, network_complete = {}, True
    for node in expected_nodes:
        rows = groups.get(node, [])
        if len(rows) < 2 or any(r.get("network_bytes") is None for r in rows):
            network_complete = False
            continue
        if not all(r.get("network_identity") for r in rows):
            # One continuously observed process can supply a delta even when the
            # namespace identity is unknown. Across a restart/node boundary the
            # lifetime counter cannot safely be attributed or de-duplicated.
            if len(expected_nodes) != 1 or len({r.get("process_identity") for r in rows}) != 1 or rows[0].get("process_identity") is None:
                network_complete = False
                continue
            network_groups["single_process_unknown_namespace"] = rows
        else:
            for identity in {r["network_identity"] for r in rows}:
                selected = [r for r in rows if r["network_identity"] == identity]
                if any(b["network_bytes"] < a["network_bytes"] for a,b in zip(selected, selected[1:])):
                    network_complete = False
                network_groups.setdefault(identity, []).extend(selected)
    known_network = 0
    for rows in network_groups.values():
        if len(rows) < 2:
            network_complete = False
        else:
            values = [r["network_bytes"] for r in rows]
            known_network += max(values)-min(values)
    result["network_known_delta_bytes"] = known_network if network_groups else None
    result["network_coverage"] = "observed_namespace_intervals" if network_complete and network_groups else "partial_or_unknown"
    result["network_bytes"] = known_network if network_complete and network_groups else None
    peaks = []
    for node in expected_nodes:
        values = [r.get("rss_bytes") for r in groups.get(node, []) if r.get("rss_bytes") is not None]
        if not values:
            break
        peaks.append(max(values))
    else:
        result["peak_rss_bytes"] = sum(peaks)  # Sum of per-node sampled maxima, not simultaneous RSS.
    # Usage is run-scoped cumulative, surviving worker restarts only if collector
    # reports the same usage identity. Keep the final report per node/identity.
    usage_rows = []
    for node in expected_nodes:
        latest = {}
        for row in groups.get(node, []):
            latest[row.get("usage_identity", row.get("process_identity", "unknown"))] = row
        if not latest:
            break
        usage_rows.extend(latest.values())
    else:
        for field in ("input_tokens", "output_tokens", "api_requests", "cost_microusd"):
            if usage_rows and all(r.get(field) is not None for r in usage_rows):
                result[field] = sum(r[field] for r in usage_rows)
        result["usage_estimated"] = any(r.get("usage_estimated", True) for r in usage_rows) if usage_rows else None
        result["cost_estimated"] = any(r.get("cost_estimated", True) for r in usage_rows) if usage_rows else None
    result["resource_scope"] = "configured run-scoped node processes; network scope specified by each collector; peak RSS is sum of node maxima"
    return result
