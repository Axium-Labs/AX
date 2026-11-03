"""Production AXCrew distributed benchmark controller; never emulates workers."""
import argparse
import base64
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import time
import uuid
from urllib.error import HTTPError
from urllib.parse import quote, urlsplit
from urllib.request import Request, urlopen
from node_control import IDENTITY, node_command, resources_summary, validate_node
from report import wilson

TERMINAL = {"completed", "failed", "cancelled"}


def request(base, token, path, body=None):
    return Client(base, token).call(path, body)


class Client:
    def __init__(self, base, token, timeout=20, max_bytes=32*1024*1024):
        self.base, self.token, self.timeout, self.max_bytes = base.rstrip("/"), token, timeout, max_bytes
        self.telemetry = []
        self.deadline = None

    def call(self, path, body=None, token=None):
        encoded = json.dumps(body).encode() if body is not None else None
        req = Request(self.base+"/api/distributed"+path, data=encoded,
            headers={"Authorization": "Bearer "+(self.token if token is None else token), "Content-Type": "application/json"})
        started, status, received = time.monotonic(), None, 0
        try:
            timeout = min(self.timeout, max(.001, self.deadline-started)) if self.deadline else self.timeout
            if self.deadline and started >= self.deadline:
                raise TimeoutError("Crew round deadline exhausted")
            with urlopen(req, timeout=timeout) as response:
                status = response.status
                raw = response.read(self.max_bytes+1)
                received = len(raw)
                if len(raw) > self.max_bytes:
                    raise ValueError("Crew response exceeded byte limit")
                return json.loads(raw)
        except HTTPError as exc:
            status = exc.code
            received = len(exc.read(self.max_bytes+1))
            exc.close()
            raise
        finally:
            self.telemetry.append(dict(path=path, status=status, payload_sent_bytes=len(encoded or b""),
                payload_received_bytes=received, wall_seconds=time.monotonic()-started))


def observed_scope(state, tasks, inventory):
    owners = {a["instance_id"] for t in tasks for a in t.get("attempts", [])}
    hosts = {state.get("instances", {}).get(i, {}).get("host_id") for i in owners}
    hosts.discard(None)
    physical = {inventory.get(h, {}).get("machine_fingerprint") for h in hosts}
    physical.discard(None)
    if len(hosts) > 1 and len(physical) == len(hosts):
        return "cross_machine_operator_attested", sorted(hosts)
    if len(hosts) > 1:
        return "multiple_host_ids_physical_identity_unverified", sorted(hosts)
    return "single_host_real_workers" if hosts else "no_worker_execution_observed", sorted(hosts)


def task_graph(items):
    aliases = [t.get("id", t.get("submit", {}).get("request_id", "")) for t in items]
    if not aliases or any(not IDENTITY.fullmatch(alias) for alias in aliases) or len(set(aliases)) != len(aliases):
        raise ValueError("tasks require unique safe id/request_id aliases")
    by_alias = dict(zip(aliases, items))
    pending, levels, seen = dict(by_alias), [], set()
    for alias, item in by_alias.items():
        deps = item.get("depends_on", [])
        if not isinstance(deps, list) or len(set(deps)) != len(deps) or any(d not in by_alias or d == alias for d in deps):
            raise ValueError(f"{alias}: invalid dependency aliases")
        if item.get("submit", {}).get("dependencies"):
            raise ValueError("use depends_on aliases, not stale server dependency IDs")
        if any(item.get("submit", {}).get(k) for k in ("parent_id", "workflow_id", "parent_generation")):
            raise ValueError("benchmark tasks cannot attach to an existing workflow or parent")
        if "expected_result_json" not in item and not item.get("artifact_checks") and not item.get("verify_command"):
            raise ValueError(f"{alias}: deterministic result/artifact/verifier assertion required")
        if not all(item.get("submit", {}).get(k) for k in ("title", "input", "project_id")):
            raise ValueError(f"{alias}: submit title/input/project_id required")
    while pending:
        ready = sorted(alias for alias, item in pending.items() if set(item.get("depends_on", [])) <= seen)
        if not ready:
            raise ValueError("task dependency cycle")
        levels.append(ready)
        seen.update(ready)
        for alias in ready:
            pending.pop(alias)
    return by_alias, levels


def validate_config(config):
    task_graph(config.get("tasks", []))
    if config.get("rounds", 3) < 3 or config.get("timeout_seconds", 0) <= 0:
        raise ValueError("requires >=3 repeats and positive timeout_seconds")
    if urlsplit(config.get("url", "")).scheme not in {"http", "https"}:
        raise ValueError("HTTP(S) Crew URL required")
    for field in ("global_seconds", "global_tokens", "global_cost_microusd", "max_cpu_seconds", "max_network_bytes", "max_rss_bytes"):
        if config.get(field, 0) < 0:
            raise ValueError(f"{field} must be nonnegative")
    for budget in [config.get("task_budget", {}), *[t.get("task_budget", {}) for t in config["tasks"]]]:
        if not isinstance(budget, dict) or set(budget)-{"max_rounds", "max_seconds", "max_tokens", "max_cost_microusd"} or any(not isinstance(v, int) or v < 0 for v in budget.values()):
            raise ValueError("task_budget supports nonnegative native max_rounds/seconds/tokens/cost_microusd only")
    nodes = config.get("nodes", [])
    if len({n.get("id") for n in nodes}) != len(nodes):
        raise ValueError("duplicate node id")
    for node in nodes:
        validate_node(node)
    known_nodes = {n["id"] for n in nodes}
    for case in config.get("scalability", [dict(nodes=sorted(known_nodes), concurrency=config.get("submission_concurrency", 4), copies=1)]):
        if not set(case.get("nodes", [])) <= known_nodes or case.get("concurrency", 0) < 1 or case.get("copies", 1) < 1:
            raise ValueError("invalid scalability node set, concurrency or copies")
        if len(case.get("nodes", [])) != len(set(case.get("nodes", []))):
            raise ValueError("duplicate node in scalability case")
    by_node = {n["id"]:n for n in nodes}
    for fault in config.get("faults", []):
        node = by_node.get(fault.get("node"))
        kind = fault.get("kind")
        restore = "reconnect" if kind == "disconnect" else "start"
        if not node or kind not in {"crash", "stop", "disconnect"} or kind not in node.get("commands", {}) or restore not in node.get("commands", {}):
            raise ValueError("each fault needs explicit owned-node fault and recovery commands")
        if fault.get("hold_seconds", 0) < 0:
            raise ValueError("fault hold_seconds must be nonnegative")


def check_consistency(previous, state, ids):
    errors = []
    if previous and state.get("revision", -1) < previous.get("revision", -1):
        errors.append("cluster_revision_regressed")
    sequences = [e["sequence"] for e in state.get("events", [])]
    if sequences != sorted(set(sequences)):
        errors.append("event_sequence_not_strictly_increasing")
    for id in ids:
        task = state.get("tasks", {}).get(id)
        old = previous.get("tasks", {}).get(id) if previous else None
        if not task:
            errors.append(f"task_disappeared:{id}")
            continue
        attempts = task.get("attempts", [])
        generations = [a["generation"] for a in attempts]
        if generations != sorted(set(generations)):
            errors.append(f"attempt_generations_invalid:{id}")
        if old and task.get("generation", 0) < old.get("generation", 0):
            errors.append(f"task_generation_regressed:{id}")
        if old:
            retained = {a["generation"]:a for a in attempts}
            for attempt in old.get("attempts", []):
                if attempt["generation"] not in retained or not set(attempt.get("sessions", [])) <= set(retained[attempt["generation"]].get("sessions", [])):
                    errors.append(f"attempt_session_history_lost:{id}:{attempt['generation']}")
        if old and old.get("status") in TERMINAL and (task.get("status"), task.get("result"), task.get("failure")) != (old.get("status"), old.get("result"), old.get("failure")):
            errors.append(f"terminal_result_changed:{id}")
        if task.get("status") == "running":
            if not attempts or (task.get("owner"), task.get("incarnation"), task.get("generation")) != (attempts[-1].get("instance_id"), attempts[-1].get("incarnation"), attempts[-1].get("generation")):
                errors.append(f"running_owner_attempt_mismatch:{id}")
        if task.get("status") == "completed" and not attempts:
            errors.append(f"completed_without_attempt:{id}")
        if task.get("status") in {"running", "completed"} and attempts:
            for dependency in task.get("spec", {}).get("dependencies", []):
                dep = state.get("tasks", {}).get(dependency, {})
                end = dep.get("attempts", [{}])[-1].get("ended_at") if dep.get("attempts") else None
                if dep.get("status") != "completed" or (end is not None and attempts[0]["started_at"] < end):
                    errors.append(f"dependency_execution_order_violation:{id}:{dependency}")
    return errors


def _atomic(path, text):
    temporary = path.with_name(path.name+".tmp")
    temporary.write_text(text, encoding="utf-8")
    os.replace(temporary, path)


def write_report(result, output):
    import csv
    import io
    rows = result["rounds"]
    attempted = [r for r in rows if r["status"] not in {"not_run", "running"}]
    passed = sum(r["status"] == "passed" for r in attempted)
    result["summary"] = dict(attempted=len(attempted), passed=passed, scheduled=len(rows),
        success_rate=passed/len(attempted) if attempted else None, wilson_95=wilson(passed, len(attempted)),
        recovered_faults=sum(f.get("recovered") is True for r in attempted for f in r.get("faults", [])),
        injected_faults=sum(f.get("injected") is True for r in attempted for f in r.get("faults", [])))
    injected = result["summary"]["injected_faults"]
    result["summary"]["recovery_rate"] = result["summary"]["recovered_faults"]/injected if injected else None
    _atomic(output/"results.json", json.dumps(result, indent=2))
    stream = io.StringIO(newline="")
    writer = csv.DictWriter(stream, fieldnames=["case", "round", "status", "reason", "scope", "wall_seconds", "throughput_tasks_per_second", "scheduling_delay_mean_seconds", "cpu_seconds", "peak_rss_bytes", "network_bytes", "input_tokens", "output_tokens", "cost_microusd"], extrasaction="ignore")
    writer.writeheader()
    writer.writerows(rows)
    _atomic(output/"runs.csv", stream.getvalue())
    _atomic(output/"report.md", "# AXCrew distributed benchmark\n\nExecution mode: "+result["metadata"]["execution_mode"]+"\n\n```json\n"+json.dumps(result, indent=2)+"\n```\n")


def node_context(node, run_id):
    return dict(run_id=run_id, node_id=node["id"], run_dir=node.get("run_directory", "/tmp/axbench/{run_id}").replace("{run_id}", run_id))


def collect_probes(nodes, run_id, folder, samples, index, deadline):
    for node in nodes:
        if "probe" not in node.get("commands", {}):
            continue
        try:
            value = node_command(node, "probe", node_context(node, run_id), folder/f"probe-{index}-{node['id']}", min(15, max(.01, deadline-time.monotonic()))).get("json")
            if not isinstance(value, dict) or value.get("run_id") != run_id:
                raise ValueError("probe did not attest current run_id")
            value["node_id"] = node["id"]
            samples.append(value)
        except Exception as exc:
            samples.append(dict(node_id=node["id"], run_id=run_id, probe_error=type(exc).__name__, timestamp=time.time()))


def budget_reason(config, resources, elapsed):
    if config.get("global_seconds", 0) and elapsed >= config["global_seconds"]:
        return "global_time_budget"
    for limit, metric in (("max_cpu_seconds", "cpu_seconds"), ("max_network_bytes", "network_bytes"), ("max_rss_bytes", "peak_rss_bytes"), ("global_tokens", "tokens"), ("global_cost_microusd", "cost_microusd")):
        if not config.get(limit, 0):
            continue
        value = resources.get(metric) if metric != "tokens" else (resources["input_tokens"]+resources["output_tokens"] if resources.get("input_tokens") is not None and resources.get("output_tokens") is not None else None)
        if value is None or (metric == "tokens" and resources.get("usage_estimated") is not False) or (metric == "cost_microusd" and resources.get("cost_estimated") is not False):
            return f"{limit}_unmetered_fail_closed"
        if value >= config[limit]:
            return limit+"_exhausted"
    return None


def download_artifacts(client, task, folder):
    files = {}
    for id in task.get("artifacts", []):
        value = client.call("/artifacts/"+quote(id))
        info = value["artifact"]
        content = base64.b64decode(value["content_base64"], validate=True)
        if len(content) > 8*1024*1024 or hashlib.sha256(content).hexdigest() != info["sha256"] or len(content) != info["size"]:
            raise ValueError("artifact checksum/size mismatch")
        path = folder/(hashlib.sha256(id.encode()).hexdigest()+".bin")
        path.write_bytes(content)
        files[id] = dict(metadata=info, path=str(path), sha256=info["sha256"])
    return files


def verify_task(item, task, client, folder, context, timeout):
    folder.mkdir(parents=True, exist_ok=True)
    checks = []
    if "expected_result_json" in item:
        try:
            valid = json.loads(task.get("result") or "null") == item["expected_result_json"]
        except ValueError:
            valid = False
        checks.append(dict(kind="result_json", passed=valid))
    artifacts = download_artifacts(client, task, folder)
    for check in item.get("artifact_checks", []):
        candidates = [f for f in artifacts.values() if all(f["metadata"].get(k) == check[k] for k in ("name", "kind") if k in check)]
        passed = False
        for candidate in candidates:
            try:
                raw = Path(candidate["path"]).read_bytes()
                valid = ("sha256" not in check or candidate["sha256"] == check["sha256"])
                if "expected_json" in check:
                    valid = valid and json.loads(raw) == check["expected_json"]
                if "expected_text" in check:
                    valid = valid and raw.decode("utf-8") == check["expected_text"]
                passed |= valid
            except (ValueError, UnicodeError):
                pass
        checks.append(dict(kind="artifact", passed=passed, name=check.get("name")))
    manifest = folder/"artifacts.json"
    manifest.write_text(json.dumps(artifacts, indent=2), encoding="utf-8")
    if item.get("verify_command"):
        verifier = dict(id="verifier", dedicated_benchmark_environment=True, commands={"verify":item["verify_command"]})
        try:
            result = node_command(verifier, "verify", dict(context, artifacts=str(manifest), task_id=task["id"]), folder/"verifier", max(.01, timeout))
            checks.append(dict(kind="trusted_artifact_verifier", passed=isinstance(result.get("json"), dict) and result["json"].get("passed") is True))
        except Exception as exc:
            checks.append(dict(kind="trusted_artifact_verifier", passed=False, reason=type(exc).__name__))
    return dict(passed=task.get("status") == "completed" and bool(checks) and all(c["passed"] for c in checks), checks=checks, artifacts=artifacts)


def checkpoint_conflicts(client, workflow_ids, run_id):
    checks = []
    for workflow_id in sorted(set(workflow_ids)):
        accepted = None
        for _ in range(3):
            workflow = client.call("/workflows/"+quote(workflow_id))["workflow"]
            value = dict(workflow.get("state", {}))
            value["ax_bench_cas_probe"] = {"run_id":run_id}
            body = dict(expected_revision=workflow["revision"], state=value)
            try:
                accepted = client.call("/workflows/"+quote(workflow_id)+"/checkpoint", body)
                break
            except HTTPError as exc:
                retryable = exc.code in {400, 409}
                exc.close()
                if not retryable:
                    raise
        rejected = False
        if accepted is not None:
            try:
                client.call("/workflows/"+quote(workflow_id)+"/checkpoint", body)
            except HTTPError as exc:
                rejected = exc.code in {400, 409}
                exc.close()
        checks.append(dict(workflow_id=workflow_id, passed=accepted is not None and rejected,
            observed_new_revision=accepted.get("revision") if accepted else None))
    return checks


def native_task_input(item, config):
    text = item["submit"]["input"]
    budget = dict(config.get("task_budget", {}), **item.get("task_budget", {}))
    if not budget:
        return text
    if text.lstrip().startswith("/goal"):
        raise ValueError("task_budget cannot wrap an already explicit /goal command")
    return "/goal create "+json.dumps(dict(budget, target=text, mode="model"), ensure_ascii=False)


def _round(config, case, repeat, row, output, client, experiment_started, previous_usage):
    from concurrent.futures import ThreadPoolExecutor
    run_id = "axb-"+uuid.uuid4().hex
    folder = output/f"case-{row['case']}-round-{repeat}"
    folder.mkdir()
    row.update(run_id=run_id, artifacts=str(folder), status="running", reason=None)
    nodes = [n for n in config.get("nodes", []) if n["id"] in case["nodes"]]
    by_node = {n["id"]:n for n in nodes}
    started = time.monotonic()
    deadline = min(started+config["timeout_seconds"], experiment_started+config["global_seconds"] if config.get("global_seconds") else float("inf"))
    client.deadline = deadline
    samples, ids, saved, faults, cleanup_errors = [], {}, {}, [], []
    row["faults"] = faults
    previous, state, probe_index = None, {}, 0
    consistency_errors, idempotency_checks, fencing_checks, receipt_checks, cas_checks = [], [], [], [], []
    pending_restores = []
    running_observations = []
    try:
        for node in nodes:
            for command in ("prepare", "start", "identity"):
                if command in node.get("commands", {}):
                    result = node_command(node, command, node_context(node, run_id), folder/f"{command}-{node['id']}", min(30, max(.01, deadline-time.monotonic())))
                    if command == "identity":
                        row.setdefault("node_identities", {})[node["id"]] = result.get("json")
                        identity = result.get("json")
                        if not isinstance(identity, dict) or identity.get("run_id") != run_id:
                            raise ValueError("node identity command must return current run_id")
                        for expected, actual in (("expected_ax_commit", "ax_commit"), ("expected_crew_commit", "crew_commit")):
                            if node.get(expected) and identity.get(actual) != node[expected]:
                                raise ValueError("node source revision differs from configured revision")
        state = client.call("")
        selected_instances = {n["instance_id"] for n in nodes if n.get("instance_id")}
        if nodes and len(selected_instances) != len(nodes):
            raise ValueError("each selected real node requires a distinct registered instance_id")
        if any(i not in state.get("instances", {}) for i in selected_instances):
            raise ValueError("configured node instance is not enrolled in Crew")
        items, levels = task_graph(config["tasks"])
        workflows = {}
        def retain(key, item, spec, group, task):
            ids[key] = task["id"]
            saved[key] = dict(item=item, spec=spec, task=task)
            workflows[group] = task["spec"]["workflow_id"]
        for copy in range(case.get("copies", 1)):
            for level in levels:
                pending = []
                for alias in level:
                    item = items[alias]
                    spec = dict(item["submit"])
                    spec["input"] = native_task_input(item, config)
                    spec.pop("request_id", None)
                    spec["request_id"] = f"{run_id}-{copy}-{alias}"
                    if len(spec["request_id"]) > 128:
                        raise ValueError("namespaced request id exceeds server bound")
                    spec["dependencies"] = [ids[f"{copy}:{d}"] for d in item.get("depends_on", [])]
                    group = (copy, item.get("workflow", spec["project_id"]))
                    if config.get("model") and not spec.get("requirements", {}).get("model"):
                        spec["requirements"] = dict(spec.get("requirements", {}), model=config["model"])
                    key = f"{copy}:{alias}"
                    if group not in workflows:
                        if selected_instances:
                            spec["collaboration"] = dict(spec.get("collaboration", {}), participants={id:"benchmark" for id in sorted(selected_instances)})
                        retain(key, item, spec, group, client.call("/tasks", spec))
                    else:
                        spec["workflow_id"] = workflows[group]
                        pending.append((key, item, spec, group))
                # Root Workflow binding is serial; sibling submissions use actual
                # bounded concurrent HTTP, and native Crew owns task execution.
                with ThreadPoolExecutor(max_workers=case["concurrency"]) as pool:
                    futures = [(record, pool.submit(client.call, "/tasks", record[2])) for record in pending]
                    errors = []
                    for (key, item, spec, group), future in futures:
                        try:
                            retain(key, item, spec, group, future.result())
                        except Exception as exc:
                            errors.append(type(exc).__name__)
                    if errors:
                        row["submission_errors"] = errors
                        raise RuntimeError("partial submission failed; retained submissions will be cancelled")
        row["configured_submission_concurrency"] = case["concurrency"]
        if config.get("check_idempotency", True):
            for key, record in saved.items():
                spec, task = record["spec"], record["task"]
                again = client.call("/tasks", spec)
                passed, conflict_rejected = again["id"] == task["id"], False
                try:
                    conflicting = client.call("/tasks", dict(spec, input=spec["input"]+"\nAX-Bench conflicting replay"))
                    if conflicting.get("id") not in ids.values():
                        ids["conflicting:"+key] = conflicting["id"]
                except HTTPError as exc:
                    conflict_rejected = exc.code in {400, 409}
                    exc.close()
                idempotency_checks.append(dict(task_id=task["id"], identical_same_id=passed, conflicting_rejected=conflict_rejected))
        if config.get("check_checkpoint_cas", False):
            cas_checks = checkpoint_conflicts(client, workflows.values(), run_id)
        while True:
            state = client.call("")
            current = [state.get("tasks", {}).get(id, {}) for id in ids.values()]
            running_observations.append({n["id"]:sum(t.get("status") == "running" and t.get("owner") == n.get("instance_id") for t in current) for n in nodes})
            consistency_errors.extend(check_consistency(previous, state, set(ids.values())))
            with (folder/"snapshots.jsonl").open("a", encoding="utf-8") as stream:
                stream.write(json.dumps(dict(timestamp=time.time(), state=state))+"\n")
            if nodes:
                collect_probes(nodes, run_id, folder, samples, probe_index, deadline)
                probe_index += 1
            resources = resources_summary(samples, {n["id"] for n in nodes})
            total_usage = dict(resources)
            for field in ("input_tokens", "output_tokens", "cost_microusd"):
                if resources.get(field) is not None:
                    total_usage[field] += previous_usage.get(field, 0)
            reason = budget_reason(config, total_usage, time.monotonic()-experiment_started)
            if reason and (len(samples) >= 2*len(nodes) or not nodes):
                row["reason"] = reason
                break
            elapsed = time.monotonic()-started
            if config.get("check_receipt_idempotency", False):
                for task in current:
                    identity = (task.get("id"), task.get("generation"))
                    node = next((n for n in nodes if n.get("instance_id") == task.get("owner")), None)
                    if task.get("status") != "running" or not node or any(tuple(c["identity"]) == identity for c in receipt_checks):
                        continue
                    worker_token = os.environ.get(node.get("worker_token_env", ""))
                    if not worker_token:
                        receipt_checks.append(dict(identity=identity, passed=None, reason="worker_token_not_configured"))
                        continue
                    body = dict(incarnation=task["incarnation"], task_id=task["id"], generation=task["generation"],
                        message_id=run_id+"-receipt-"+task["id"], kind="observation", text="AX-Bench idempotent observation "+run_id+task["id"], artifacts=[])
                    rejected, replayed = False, False
                    try:
                        client.call("/worker/report", body, token=worker_token)
                        client.call("/worker/report", body, token=worker_token)
                        replayed = True
                        try:
                            client.call("/worker/report", dict(body, text=body["text"]+" conflict"), token=worker_token)
                        except HTTPError as exc:
                            rejected = exc.code in {400, 409}
                            exc.close()
                    except HTTPError as exc:
                        exc.close()
                    observed = client.call("")
                    occurrences = sum(e.get("task_id") == task["id"] and e.get("kind") == "task.observation" and e.get("detail") == body["text"] for e in observed.get("events", []))
                    receipt_checks.append(dict(identity=identity, passed=replayed and rejected and occurrences == 1,
                        identical_replayed=replayed, conflicting_rejected=rejected, observed_event_count=occurrences))
            for plan_index, plan in enumerate(config.get("faults", [])):
                if plan["node"] not in by_node or any(f["plan_index"] == plan_index for f in faults):
                    continue
                targets = [t for t in current if t.get("status") == "running" and t.get("owner") == by_node[plan["node"]].get("instance_id")]
                if elapsed < plan.get("after_seconds", 0) or (plan.get("require_running", True) and not targets):
                    continue
                if plan.get("task"):
                    allowed = {id for key,id in ids.items() if key.endswith(":"+plan["task"])}
                    targets = [t for t in targets if t["id"] in allowed]
                    if not targets:
                        continue
                node = by_node[plan["node"]]
                restore = "reconnect" if plan["kind"] == "disconnect" else "start"
                receipt = dict(plan_index=plan_index, node=node["id"], kind=plan["kind"], injected=False,
                    started_seconds=elapsed, injected_at=time.time(), affected=[dict(task_id=t["id"], owner=t["owner"], incarnation=t["incarnation"], generation=t["generation"]) for t in targets], recovered=None)
                receipt["published_artifacts_before_fault"] = {id:a["sha256"] for id,a in state.get("artifacts", {}).items() if a.get("task_id") in ids.values()}
                faults.append(receipt)
                pending_restores.append((node, restore, time.monotonic()+plan.get("hold_seconds", 0), receipt))
                node_command(node, plan["kind"], node_context(node, run_id), folder/f"fault-{plan_index}", min(20, max(.01, deadline-time.monotonic())))
                receipt["injected"] = True
            for restore in list(pending_restores):
                node, command, when, receipt = restore
                if time.monotonic() >= when:
                    node_command(node, command, node_context(node, run_id), folder/f"restore-{receipt['plan_index']}", min(20, max(.01, deadline-time.monotonic())))
                    receipt["restored_seconds"] = time.monotonic()-started
                    pending_restores.remove(restore)
            # Stale OBSERVATION probes cannot fabricate completion. They address
            # only owned benchmark task identities captured before an actual fault.
            for fault in faults:
                for old in fault["affected"]:
                    task = state.get("tasks", {}).get(old["task_id"], {})
                    probe_key = (old["task_id"], old["generation"])
                    if task.get("generation", 0) <= old["generation"] or any(tuple(x["identity"]) == probe_key for x in fencing_checks):
                        continue
                    worker_token = os.environ.get(by_node[fault["node"]].get("worker_token_env", ""))
                    if not worker_token:
                        fencing_checks.append(dict(identity=probe_key, passed=None, reason="worker_token_not_configured"))
                        continue
                    rejected = False
                    try:
                        who = client.call("/worker/identity", token=worker_token)
                        if who.get("instance_id") != old["owner"]:
                            fencing_checks.append(dict(identity=probe_key, passed=None, reason="worker_identity_mismatch"))
                            continue
                        client.call("/worker/report", dict(incarnation=old["incarnation"], task_id=old["task_id"], generation=old["generation"],
                            message_id=run_id+"-stale-"+str(old["generation"]), kind="observation", text="AX-Bench stale execution observation probe", artifacts=[]), token=worker_token)
                    except HTTPError as exc:
                        rejected = exc.code in {400, 409}
                        exc.close()
                    fencing_checks.append(dict(identity=probe_key, passed=rejected))
            previous = state
            all_terminal = current and all(t.get("status") in TERMINAL for t in current)
            if all_terminal and not pending_restores:
                break
            if time.monotonic() >= deadline:
                row["reason"] = "task_timeout"
                break
            time.sleep(min(config.get("poll_seconds", 1), max(.01, deadline-time.monotonic())))
        assertions = []
        for key, record in saved.items():
            task = state.get("tasks", {}).get(ids[key], record["task"])
            verification = verify_task(record["item"], task, client, folder/("verify-"+key.replace(":", "-")), dict(run_id=run_id, run_dir=str(folder)), min(30, max(.01, deadline-time.monotonic())))
            attempts = task.get("attempts", [])
            assertions.append(dict(alias=key, id=task["id"], status=task["status"], passed=verification["passed"], verification=verification,
                attempts=attempts, failure=task.get("failure"), scheduling_delay_seconds=attempts[0]["started_at"]-task["created_at"] if attempts else None))
        for fault in faults:
            outcomes = []
            before = fault.get("published_artifacts_before_fault", {})
            fault["data_loss"] = any(state.get("artifacts", {}).get(id, {}).get("sha256") != sha for id,sha in before.items()) if before else None
            fault["data_loss_scope"] = "pre-fault published benchmark artifact metadata; raw Session filesystem loss requires an independent node verifier"
            for old in fault["affected"]:
                task = state.get("tasks", {}).get(old["task_id"], {})
                assertion = next((a for a in assertions if a["id"] == old["task_id"]), {})
                resumed = task.get("generation", 0) > old["generation"]
                reassigned = any(a["generation"] > old["generation"] and a["instance_id"] != old["owner"] for a in task.get("attempts", []))
                passed = assertion.get("passed", False) and resumed
                if config["faults"][fault["plan_index"]].get("require_reassignment", False):
                    passed = passed and reassigned
                ends = [a.get("ended_at") for a in task.get("attempts", []) if a["generation"] > old["generation"] and a.get("ended_at")]
                outcomes.append(dict(task_id=old["task_id"], resumed=resumed, reassigned=reassigned, passed=passed,
                    recovery_seconds=max(0, max(ends)-fault["injected_at"]) if ends else None))
            fault["outcomes"] = outcomes
            fault["recovered"] = bool(outcomes) and all(t["passed"] for t in outcomes) and fault["data_loss"] is not True
        current = [state["tasks"][id] for id in ids.values() if id in state.get("tasks", {})]
        scope, hosts = observed_scope(state, current, config.get("inventory", {}))
        fault_plans = [p for p in config.get("faults", []) if p["node"] in by_node]
        faults_ok = len(faults) == len(fault_plans) and all(f["injected"] and f["recovered"] for f in faults)
        fencing_ok = not config.get("require_fencing", False) or (bool(fencing_checks) and all(f["passed"] is True for f in fencing_checks))
        checks_ok = bool(assertions) and all(t["passed"] for t in assertions) and not consistency_errors and all(c["identical_same_id"] and c["conflicting_rejected"] for c in idempotency_checks) and faults_ok and fencing_ok
        if config.get("check_checkpoint_cas", False):
            checks_ok = checks_ok and bool(cas_checks) and all(c["passed"] for c in cas_checks)
        if config.get("check_receipt_idempotency", False):
            checks_ok = checks_ok and bool(receipt_checks) and all(c["passed"] is True for c in receipt_checks)
        if config.get("require_cross_machine", False):
            checks_ok = checks_ok and scope == "cross_machine_operator_attested"
        resources = resources_summary(samples, {n["id"] for n in nodes})
        if row.get("reason") is None:
            final_usage = dict(resources)
            for field in ("input_tokens", "output_tokens", "cost_microusd"):
                if final_usage.get(field) is not None:
                    final_usage[field] += previous_usage.get(field, 0)
            row["reason"] = budget_reason(config, final_usage, time.monotonic()-experiment_started)
        delays = [a["scheduling_delay_seconds"] for a in assertions if a["scheduling_delay_seconds"] is not None]
        row.update(tasks=assertions, scope=scope, hosts=hosts, status="passed" if checks_ok and row.get("reason") is None else "failed",
            reason=row.get("reason") or (None if checks_ok else "task_fault_or_consistency_assertion_failed"),
            wall_seconds=time.monotonic()-started, throughput_tasks_per_second=sum(t["passed"] for t in assertions)/max(.000001, time.monotonic()-started),
            scheduling_delay_mean_seconds=sum(delays)/len(delays) if delays else None,
            consistency=dict(passed=not consistency_errors, failures=sorted(set(consistency_errors)), idempotency=idempotency_checks, stale_execution_fencing=fencing_checks,
                checkpoint_cas=cas_checks, receipt_idempotency=receipt_checks), **resources)
        row["sampled_concurrent_running_max"] = max((sum(o.values()) for o in running_observations), default=None) if nodes else None
        row["node_busy_fraction"] = {n["id"]:sum(o[n["id"]] > 0 for o in running_observations)/len(running_observations) for n in nodes} if running_observations else {}
        row["resolved_models"] = [json.loads(v) for v in sorted({json.dumps(a["model"], sort_keys=True) for t in current for a in t.get("attempts", []) if a.get("model")})]
        measured_loss = [f["data_loss"] for f in faults if f.get("data_loss") is not None]
        row["data_loss"] = any(measured_loss) if measured_loss else None
    except KeyboardInterrupt:
        row.update(status="interrupted", reason="controller_interrupted", wall_seconds=time.monotonic()-started)
        raise
    except Exception as exc:
        import traceback
        (folder/"controller-error.txt").write_text(traceback.format_exc(), encoding="utf-8")
        row.update(status="failed", reason=f"controller_error:{type(exc).__name__}", wall_seconds=time.monotonic()-started)
    finally:
        client.deadline = None  # Cleanup has its own bounded commands and requests.
        for node, command, _, receipt in pending_restores:
            try:
                node_command(node, command, node_context(node, run_id), folder/f"cleanup-restore-{receipt['plan_index']}", 20)
            except Exception as exc:
                cleanup_errors.append(f"restore:{node['id']}:{type(exc).__name__}")
        for id in ids.values():
            try:
                task = client.call("/tasks/"+quote(id))["task"]
                if task.get("status") not in TERMINAL:
                    client.call("/tasks/"+quote(id)+"/cancel", {})
            except Exception as exc:
                cleanup_errors.append(f"cancel:{id}:{type(exc).__name__}")
        for node in reversed(nodes):
            if "cleanup" in node.get("commands", {}):
                try:
                    node_command(node, "cleanup", node_context(node, run_id), folder/f"cleanup-{node['id']}", 20)
                except Exception as exc:
                    cleanup_errors.append(f"cleanup:{node['id']}:{type(exc).__name__}")
        row["cleanup_errors"] = cleanup_errors
        row["submitted_task_ids"] = list(ids.values())
        (folder/"resources.json").write_text(json.dumps(samples, indent=2), encoding="utf-8")
        row["coordination_http"] = dict(requests=len(client.telemetry),
            payload_sent_bytes=sum(t["payload_sent_bytes"] for t in client.telemetry),
            payload_received_bytes=sum(t["payload_received_bytes"] for t in client.telemetry),
            request_wall_seconds=sum(t["wall_seconds"] for t in client.telemetry), scope="controller_Crew_HTTP_payloads_excluding_headers_TLS_worker_traffic")
        (folder/"http.json").write_text(json.dumps(client.telemetry, indent=2), encoding="utf-8")
        if cleanup_errors:
            row.update(status="failed", reason="cleanup_failed")
        row["wall_seconds"] = time.monotonic()-started


def run_experiment(config, output, execute=False, round_numbers=None):
    """Run the real-node matrix or write a plan; returns the complete report.

    round_numbers is reserved for a parent Runner managing >=3 repetitions; it
    selects its own outer repetition without multiplying the number of paid runs.
    """
    validate_config(config)
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    if (output/"results.json").exists():
        raise ValueError("refusing to overwrite previous experiment")
    token = os.environ.get(config.get("token_env", "AX_BENCH_CREW_TOKEN"), "")
    if execute and (not token or not config.get("dedicated_benchmark_environment")):
        raise ValueError("execution needs token and dedicated_benchmark_environment=true")
    rounds = list(range(1, config.get("rounds", 3)+1)) if round_numbers is None else list(round_numbers)
    if not rounds or len(set(rounds)) != len(rounds) or any(not isinstance(r, int) or r < 1 for r in rounds):
        raise ValueError("round_numbers must contain positive unique repetition numbers")
    cases = config.get("scalability") or [dict(nodes=[n["id"] for n in config.get("nodes", [])], concurrency=config.get("submission_concurrency", 4), copies=1)]
    import platform
    import sys
    controller_files = [Path(__file__), Path(__file__).with_name("node_control.py"), Path(__file__).with_name("resource_probe.py"), Path(__file__).with_name("worker_control.py")]
    result = dict(metadata=dict(schema_version=2, execution_mode="production_crew_rest" if execute else "offline_plan",
        repetition_scope="parent_runner_managed" if round_numbers is not None else "standalone_three_or_more",
        created_utc=datetime.now(timezone.utc).isoformat(), config_sha256=hashlib.sha256(json.dumps(config, sort_keys=True).encode()).hexdigest(),
        crew_commit=config.get("crew_commit"), ax_commit=config.get("ax_commit"), model=config.get("model"),
        inventory=config.get("inventory", {}), node_identity_source="operator configuration plus optional run-scoped identity commands", matrix=cases),
        rounds=[dict(case=i+1, round=r, status="not_run", reason="offline_plan", tasks=[]) for i,_ in enumerate(cases) for r in rounds],
        limitations=["No simulated workers. Physical fingerprints are operator attestation, not independent proof of machine location.",
            "Resources come only from configured run-scoped probes; null is unavailable, not zero. Linux network counters need an isolated network namespace for attribution.",
            "Provider usage requires run-scoped collectors. Global cost/token caps stop admission/cancel tasks after observations; in-flight calls can overshoot.",
            "State assertions cover observed durable snapshots, attempt generations, event ordering, idempotency and configured stale observations; they are not a proof of all distributed histories.",
            "Node lifecycle/fault commands are explicit dedicated-environment scripts addressed by run_id. No cloud provisioning or arbitrary user process termination is inferred."])
    result["metadata"]["controller"] = dict(platform=platform.platform(), python=sys.version,
        files_sha256={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in controller_files})
    from run import source_identity
    result["metadata"]["benchmark_source"] = source_identity()
    result["metadata"]["configured_commit_provenance"] = "operator_declared; node identity commands may assert actual values"
    result["metadata"]["task_budget"] = config.get("task_budget")
    started, usage, global_stop = time.monotonic(), {}, None
    write_report(result, output)
    try:
        for row in result["rounds"]:
            if not execute:
                continue
            if global_stop:
                row["reason"] = global_stop
                continue
            if config.get("global_seconds") and time.monotonic()-started >= config["global_seconds"]:
                global_stop = row["reason"] = "global_time_budget"
                continue
            client = Client(config["url"], token, min(20, config["timeout_seconds"]))
            _round(config, cases[row["case"]-1], row["round"], row, output, client, started, usage)
            for field in ("input_tokens", "output_tokens", "cost_microusd"):
                if row.get(field) is not None:
                    usage[field] = usage.get(field, 0)+row[field]
            resources = dict(row)
            resources.update(usage)
            global_stop = budget_reason(config, resources, time.monotonic()-started)
            write_report(result, output)
    finally:
        result["metadata"]["elapsed_seconds"] = time.monotonic()-started
        write_report(result, output)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = run_experiment(json.loads(args.config.read_text(encoding="utf-8-sig")), args.output, args.execute)
    return 1 if any(r["status"] in {"failed", "interrupted"} for r in result["rounds"]) else 0


if __name__ == "__main__":
    raise SystemExit(main())
