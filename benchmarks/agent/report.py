"""Capability reporting. Uses Phase 1 nearest-rank statistics; missing != zero."""
import csv
import json
import math
import os
from collections import Counter, defaultdict
from execution import phase1


def wilson(successes, n):
    if not n:
        return None
    z = 1.959963984540054
    center = (successes/n+z*z/(2*n))/(1+z*z/n)
    half = z*math.sqrt(successes/n*(1-successes/n)/n+z*z/(4*n*n))/(1+z*z/n)
    return [max(0, center-half), min(1, center+half)]


def summarize(rows):
    attempted = [r for r in rows if r["status"] not in {"not_run", "skipped", "running"}]
    scored = [r for r in attempted if r["status"] not in {"ungraded", "unscorable"} and r.get("score_eligible", True)]
    successes = sum(r["status"] == "passed" for r in scored)
    durations = [dict(ns_per_op=r["wall_seconds"]*1e9, elapsed_ns=r["wall_seconds"]*1e9, operations=1) for r in attempted]
    def measured_total(key):
        known = [r.get(key) for r in attempted if r.get(key) is not None]
        return dict(known_total=sum(known) if known else None, measured_runs=len(known), total_runs=len(attempted))
    costs = measured_total("cost_microusd")
    recoveries = [r for r in scored if r.get("fault_injected")]
    completed_tools = sum(r.get("tool_finished") or 0 for r in attempted)
    tool_failures = sum(r.get("tool_failures") or 0 for r in attempted)
    security = [r["security"] for r in attempted if isinstance(r.get("security"), dict)]
    attack_calls = sum(s.get("runtime_attack_attempts", 0) for s in security)
    denied_calls = sum(s.get("typed_denial_count", 0) for s in security)
    return dict(scheduled=len(rows), attempted=len(attempted), passed=successes,
        not_run=len(rows)-len(attempted), scored_runs=len(scored), unscored_runs=len(attempted)-len(scored),
        success_rate=successes/len(scored) if scored else None,
        success_wilson_95=wilson(successes, len(scored)),
        latency=phase1.summary(durations) if durations and sum(d["elapsed_ns"] for d in durations)>0 else None,
        successful_tasks_per_second=successes/sum(r["wall_seconds"] for r in attempted) if attempted and sum(r["wall_seconds"] for r in attempted)>0 else None,
        input_tokens=measured_total("input_tokens"), output_tokens=measured_total("output_tokens"),
        api_requests=measured_total("api_requests"), cost_microusd=costs,
        tool_calls=measured_total("tool_calls"), duplicate_calls=measured_total("duplicate_call_count"),
        tool_success_rate=(completed_tools-tool_failures)/completed_tools if completed_tools else None,
        requirement_coverage_mean=sum(r["requirement_coverage"] for r in attempted if r.get("requirement_coverage") is not None)/sum(r.get("requirement_coverage") is not None for r in attempted) if any(r.get("requirement_coverage") is not None for r in attempted) else None,
        injected_fault_attempts=len(recoveries), recovery_success_rate=sum(r.get("recovery_success") is True for r in recoveries)/len(recoveries) if recoveries else None,
        recovery_wilson_95=wilson(sum(r.get("recovery_success") is True for r in recoveries), len(recoveries)),
        security=dict(observed_vectors=len(security), runtime_attack_attempts=attack_calls, typed_denials=denied_calls,
            runtime_interception_rate=denied_calls/attack_calls if attack_calls else None,
            synthetic_secret_leaks=sum(s.get("synthetic_secret_leaked") is True for s in security) if security else None,
            exfiltration_attempts=sum(s.get("exfiltration_attempted") is True for s in security) if security else None),
        cost_per_success_microusd=costs["known_total"]/successes if successes and costs["measured_runs"] == len(attempted) and len(scored) == len(attempted) else None,
        cost_scope="mixed estimates/provider billing; inspect per-run cost_estimated" if any(r.get("cost_estimated") for r in attempted) else "provider billing when available",
        failure_reasons=dict(Counter(r.get("reason", "unspecified") for r in scored if r["status"] != "passed")),
        unscored_reasons=dict(Counter(r.get("reason", "unspecified") for r in attempted if r not in scored)))


def report(result, output):
    output.mkdir(parents=True, exist_ok=True)
    result["summary"] = summarize(result["runs"])
    groups = defaultdict(list)
    tasks = defaultdict(list)
    for row in result["runs"]:
        key = f"{row['profile']}/{row['category']}"
        groups[key].append(row)
        tasks[f"{row['profile']}/{row['task']}"].append(row)
    result["groups"] = {key:summarize(rows) for key,rows in groups.items()}
    result["task_statistics"] = {key:summarize(rows) for key,rows in tasks.items()}
    temporary = output/"results.json.tmp"
    temporary.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    os.replace(temporary, output/"results.json")
    fields = ["task", "profile", "category", "round", "status", "score_eligible", "reason", "wall_seconds", "requirement_coverage", "tool_calls", "tool_failures", "duplicate_call_count", "input_tokens", "output_tokens", "usage_estimated", "api_requests", "cost_microusd", "cost_estimated", "cpu_seconds", "peak_rss_bytes", "network_bytes", "fault_injected", "recovery_success", "recovery_seconds", "data_loss", "session_history_data_loss", "rotations", "compression_count", "runtime_interception_rate", "synthetic_secret_leaked", "artifacts"]
    with (output/"runs.csv.tmp").open("w", newline="", encoding="utf-8") as stream:
        writer = csv.DictWriter(stream, fieldnames=fields, extrasaction="ignore")
        writer.writeheader()
        writer.writerows(dict(row, **{k:row.get("security", {}).get(k) for k in ("runtime_interception_rate", "synthetic_secret_leaked")}) for row in result["runs"])
    os.replace(output/"runs.csv.tmp", output/"runs.csv")
    s = result["summary"]
    lines = ["# AX-Bench Agent Capability / Advanced", "", f"Execution mode: {result['metadata']['execution_mode']}", "",
        f"Scheduled: {s['scheduled']}; attempted: {s['attempted']}; passed: {s['passed']}; not run: {s['not_run']}.",
        f"Success rate: {s['success_rate']}; Wilson 95% interval: {s['success_wilson_95']}.", "",
        "Intervals describe observed task-attempts; repeats share task difficulty and are not independent population samples. Per-task results are also provided; no model ranking or experiment group comparison is inferred.", "",
        "## Measurements", "", "```json", json.dumps(s, indent=2), "```", "", "## Attempts", ""]
    for row in result["runs"]:
        lines.append(f"- {row['profile']} / {row['task']} / repeat {row['round']}: {row['status']}; {row.get('reason', '')}; seconds={row.get('wall_seconds')}; artifacts={row.get('artifacts')}")
    lines += ["", "## Measurement limits", "", *[f"- {item}" for item in result["limitations"]], "", "## Provenance", "", "```json", json.dumps(result["metadata"], ensure_ascii=False, indent=2), "```", ""]
    (output/"report.md.tmp").write_text("\n".join(lines), encoding="utf-8")
    os.replace(output/"report.md.tmp", output/"report.md")
