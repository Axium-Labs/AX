"""Shared production-process adapter, Phase 1 telemetry and statistical primitives."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import hashlib
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
spec = importlib.util.spec_from_file_location("phase1", HERE.parent/"runtime"/"run.py")
phase1 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(phase1)


def stop_tree(process):
    if process.poll() is not None:
        return
    if os.name == "nt":
        subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"], capture_output=True)
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    process.wait(timeout=10)


def launch(argv, workspace, env, folder, timeout, kill_after=None, preserve_paths=(), history_root=None):
    folder.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    readings, reason, preserved, history = [], None, {}, {}
    with (folder/"stdout.txt").open("wb") as out, (folder/"stderr.txt").open("wb") as err:
        proc = subprocess.Popen(argv, cwd=workspace, env=env, stdout=out, stderr=err,
            stdin=subprocess.DEVNULL, start_new_session=os.name != "nt")
        try:
            while proc.poll() is None:
                try:
                    readings.append(phase1.resources(proc))
                except OSError:
                    readings.append((None, None, None))
                elapsed = time.monotonic()-started
                if elapsed >= timeout:
                    reason = "task_timeout"
                    stop_tree(proc)
                    break
                if kill_after is not None and elapsed >= kill_after:
                    for relative in preserve_paths:
                        path = (workspace/relative).resolve()
                        if path.is_relative_to(workspace.resolve()) and path.is_file():
                            preserved[relative] = hashlib.sha256(path.read_bytes()).hexdigest()
                    if history_root is not None and history_root.exists():
                        for path in sorted(history_root.rglob("*.jsonl")):
                            if path.is_file() and path.resolve().is_relative_to(history_root.resolve()):
                                with path.open("rb") as stream:
                                    # Capture the last complete record boundary; an in-flight
                                    # partial append is not committed Session data.
                                    payload = stream.read(10_000_000)
                                end = payload.rfind(b"\n")+1
                                if end:
                                    history[str(path.relative_to(history_root))] = dict(bytes=end, sha256=hashlib.sha256(payload[:end]).hexdigest())
                    reason = "injected_process_crash"
                    stop_tree(proc)
                    break
                time.sleep(.02)
        finally:
            stop_tree(proc)
        try:
            readings.append(phase1.resources(proc))
        except OSError:
            pass
    def peak(i):
        return max((r[i] for r in readings if r[i] is not None), default=None)
    return dict(exit_code=proc.returncode, wall_seconds=time.monotonic()-started,
                stop_reason=reason, cpu_seconds=peak(0), rss_sampled_max_bytes=peak(1),
                peak_rss_bytes=peak(2), network_bytes=None,
                preserved_before_fault=preserved,
                history_before_fault=history,
                resource_scope="AX parent process only; child resources and network unavailable", argv=argv)


def session_id(data):
    db = data/"memory.sqlite3"
    if not db.exists():
        return None
    with sqlite3.connect(f"{db.as_posix()}?mode=ro", uri=True) as conn:
        row = conn.execute("SELECT id FROM sessions ORDER BY created_at DESC,rowid DESC LIMIT 1").fetchone()
        return row[0] if row else None


def trace_metrics(path):
    events, errors = [], []
    paths = path if isinstance(path, (list, tuple)) else [path]
    for source in paths:
        if not source.exists():
            continue
        for n, line in enumerate(source.read_text(encoding="utf-8").splitlines()):
            try:
                row = json.loads(line)
                if not isinstance(row.get("event"), dict) or not isinstance(row["event"].get("type"), str):
                    raise ValueError("invalid event envelope")
                row["_source"] = str(source)
                events.append(row)
            except (ValueError, AttributeError):
                errors.append(f"invalid event line {source.name}:{n+1}")
    starts = [x["event"] for x in events if x["event"]["type"] == "tool_started"]
    finishes = [x["event"] for x in events if x["event"]["type"] == "tool_finished"]
    fingerprints = [json.dumps([s["name"], s["input"]], sort_keys=True) for s in starts]
    # Goal usage is cumulative within a Goal. Sum only last snapshot per identity.
    latest = {}
    for x in events:
        e = x["event"]
        if e["type"] == "goal_updated":
            latest[e.get("goal_id") or f"ordinary:{x['_source']}"] = e["goal"].get("usage", {})
    usages = list(latest.values())
    def total(field):
        return sum(u[field] for u in usages) if usages and all(u.get(field) is not None for u in usages) else None
    failed = sum(not x["success"] for x in finishes)
    successful_ids = {x["id"] for x in finishes if x["success"]}
    mcp_calls = sum(s["id"] in successful_ids and (s["name"].startswith("mcp__bench__") or (s["name"] == "mcp" and s["input"].get("action") == "call" and s["input"].get("server") == "bench")) for s in starts)
    consecutive, maximum, previous = 0, 0, None
    for value in fingerprints:
        consecutive = consecutive+1 if value == previous else 1
        maximum = max(maximum, consecutive)
        previous = value
    goal_events = [x["event"]["goal"] for x in events if x["event"]["type"] == "goal_updated"]
    return dict(event_count=len(events), trace_errors=errors,
        tool_calls=len(starts), tool_finished=len(finishes), tool_failures=failed,
        mcp_executed_calls=mcp_calls,
        tool_success_rate=(len(finishes)-failed)/len(finishes) if finishes else None,
        duplicate_call_count=len(fingerprints)-len(set(fingerprints)),
        max_consecutive_calls=maximum,
        goal_states=[g.get("state") for g in goal_events],
        goal_stop_reasons=[g["stop_reason"] for g in goal_events if g.get("stop_reason")],
        last_goal_id=next((x["event"].get("goal_id") for x in reversed(events) if x["event"]["type"] == "goal_updated" and x["event"].get("goal_id")), None),
        compression_count=sum(x["event"]["type"] == "context_compressed" for x in events),
        invalid_call_count=None, tool_selection_accuracy=None, tool_argument_accuracy=None,
        parallel_tool_efficiency=None,
        # Duplicates are observations, never automatically classified as invalid.
        input_tokens=total("input_tokens"), output_tokens=total("output_tokens"),
        total_tokens=(total("input_tokens")+total("output_tokens")) if total("input_tokens") is not None and total("output_tokens") is not None else None,
        api_requests=total("requests"), cost_microusd=total("cost_microusd"),
        usage_estimated=any(u.get("estimated", True) for u in usages) if usages else None,
        cost_estimated=any(u.get("cost_estimated", True) for u in usages) if usages else None,
        resolved_models=sorted({(x["event"]["provider"], x["event"]["model"]) for x in events if x["event"]["type"] == "model_started"}),
        compressions=[x["event"] for x in events if x["event"]["type"] == "context_compressed"],
        rotations=sum(x["event"]["type"] == "session_rotated" for x in events),
        normal_completion=any(x["event"]["type"] == "turn_finished" for x in events),
        recovery_retries=sum(x["event"]["type"] == "model_connection" and x["event"].get("status") == "retrying" for x in events))
