"""AX-Bench Phase 2/3. Planning is offline; --execute always invokes production AX."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time
import traceback
from contextlib import ExitStack

from dataset import VERSION, catalog
from evaluate import confined, evaluate
from execution import HERE, ROOT, launch, session_id, trace_metrics
from report import report

LIMITATIONS = [
    "Only executed attempts count toward scores. Planning, skipped prerequisites and offline controller tests are not Agent results.",
    "Requirement coverage counts declared deterministic checks, not full semantic requirements or subjective quality.",
    "Provider model ids are recorded from ModelStarted; exact vendor snapshot revision may be undisclosed.",
    "Tokens/cost are Goal telemetry: missing remains null; estimated flags retained. No invoice or pricing assumptions.",
    "Native token/cost caps require complete Provider usage/billing; requests in flight may overshoot. Global limits stop admission after measured consumption; they are not prepaid billing guarantees.",
    "CPU/RSS sample only the AX parent process; child-process CPU, network and remote-node resources are unavailable unless supplied by node collection.",
    "Repeated tool calls are counted but are not automatically invalid; tool selection/argument accuracy and parallel efficiency require task-specific trace annotations and remain null.",
    "Recovery is scored only when a fault was actually injected and independently validated; repairing defective input programs is not API fault tolerance.",
    "Long task duration and actual compression/rotation counts are observations, not proof of long-horizon robustness. Security tasks require external isolated execution, and are not run by this host adapter.",
    "Public benchmark import requires pinned upstream dataset, environment and official validators. Custom tasks are not SWE-bench or Terminal-Bench scores.",
]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_identity():
    def git(*args):
        return subprocess.check_output(["git", *args], cwd=ROOT).decode("utf-8", errors="replace").strip()
    paths = sorted(p for base in (ROOT/"crates", HERE, HERE.parent/"runtime") for p in base.rglob("*") if p.is_file() and p.suffix in {".rs", ".py", ".toml", ".lock"} and not any(x in p.parts for x in ("target", "results", "__pycache__")))
    return dict(commit=git("rev-parse", "HEAD"), dirty=bool(git("status", "--porcelain")),
        diff_sha256=hashlib.sha256(subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=ROOT)).hexdigest(),
        source_sha256={str(p.relative_to(ROOT)): digest(p) for p in paths},
        lockfile_sha256=digest(ROOT/"Cargo.lock"))


def validate_config(config, require_home=True):
    if not config.get("profiles"):
        raise ValueError("at least one profile required")
    ids = set()
    for profile in config["profiles"]:
        required = ("id", "provider", "model", "home_template") if require_home else ("id", "provider", "model")
        if not all(profile.get(k) for k in required):
            raise ValueError("profile requires "+", ".join(required))
        if profile["id"] in ids or not profile["id"].replace("-", "").replace("_", "").isalnum():
            raise ValueError("duplicate or unsafe profile id")
        ids.add(profile["id"])
    for key in ("task_seconds", "max_steps", "max_tool_calls"):
        if not isinstance(config.get(key), int) or config[key] <= 0:
            raise ValueError(f"{key} must be a positive integer")
    if not isinstance(config.get("allow_dangerous", False), bool):
        raise ValueError("allow_dangerous must be an explicit boolean")
    for key in ("global_seconds", "global_tokens", "global_cost_microusd", "task_tokens", "task_cost_microusd"):
        if not isinstance(config.get(key, 0), int) or config.get(key, 0) < 0:
            raise ValueError(f"{key} must be a nonnegative integer (0 means unlimited)")


def prepare_home(template, destination):
    destination.mkdir()
    # Deliberately exclude sessions, memories, permissions, hooks, skills and MCP.
    for name in ("config.json", "auth.json", "providers.json"):
        source = template/name
        if source.exists():
            shutil.copy2(source, destination/name)
            if os.name != "nt":
                (destination/name).chmod(0o600)
    # Configuration is private local evidence, never emitted into reports.
    return {name: digest(destination/name) for name in ("config.json", "auth.json", "providers.json") if (destination/name).exists()}


def execute_crew(task, profile, config, folder, remaining_seconds):
    binding = config.get("crew")
    if not binding:
        return dict(status="not_run", reason="configure crew with a dedicated Crew experiment JSON path", wall_seconds=0)
    crew_config = json.loads(Path(binding).read_text(encoding="utf-8-sig")) if isinstance(binding, str) else dict(binding)
    if not crew_config.get("dedicated_benchmark_environment") or not os.environ.get(crew_config.get("token_env", "AX_BENCH_CREW_TOKEN")):
        return dict(status="not_run", reason="dedicated Crew environment and configured token required", wall_seconds=0)
    selected_model = {"provider": profile["provider"], "model": profile["model"]}
    configured_models = [crew_config.get("model")]+[t.get("submit", {}).get("requirements", {}).get("model") for t in crew_config.get("tasks", [])]
    if any(m is not None and m != selected_model for m in configured_models):
        return dict(status="not_run", reason="Crew model configuration differs from selected profile", wall_seconds=0)
    crew_config["model"] = selected_model
    crew_config["timeout_seconds"] = min(crew_config.get("timeout_seconds", config["task_seconds"]), config["task_seconds"], remaining_seconds)
    if crew_config["timeout_seconds"] <= 0:
        return dict(status="not_run", reason="global_time_budget", wall_seconds=0)
    from crew import run_experiment
    start = time.monotonic()
    child = run_experiment(crew_config, folder, execute=True, round_numbers=[task.get("_repeat", 1)])
    rows = child["rounds"]
    executed = [r for r in rows if r["status"] not in {"not_run", "skipped"}]
    failed = any(r["status"] in {"failed", "interrupted"} for r in executed)
    status = "not_run" if not executed else "failed" if failed else "passed" if len(executed) == len(rows) and all(r["status"] == "passed" for r in executed) else "ungraded"
    result = dict(status=status, reason=None if status == "passed" else "Crew experiment has failed, ungraded or unavailable cases; inspect child report",
                  wall_seconds=time.monotonic()-start, crew_report=str(folder/"results.json"), crew_summary=child.get("summary"),
                  scope=sorted({r.get("scope", "unavailable") for r in rows}), score_eligible=status not in {"ungraded", "not_run"})
    injected = child.get("summary", {}).get("injected_faults", 0)
    recovered = child.get("summary", {}).get("recovered_faults", 0)
    result.update(fault_injected=injected > 0, recovery_success=(injected == recovered) if injected else None,
                  distributed_fault_count=injected, distributed_recovered_fault_count=recovered)
    for field in ("input_tokens", "output_tokens", "api_requests", "cost_microusd", "cpu_seconds", "network_bytes"):
        values = [r.get(field) for r in executed]
        result[field] = sum(values) if values and all(isinstance(v, (int, float)) for v in values) else None
    result["usage_estimated"] = any(r.get("usage_estimated") is not False for r in executed) if executed else None
    result["cost_estimated"] = any(r.get("cost_estimated") is not False for r in executed) if executed else None
    return result


def execute_task(task, profile, config, binary, folder, remaining_seconds):
    if task.get("executor") == "security":
        from security import execute_security
        return execute_security(task, profile, config, binary, folder, remaining_seconds)
    if task.get("executor") == "crew":
        return execute_crew(task, profile, config, folder, remaining_seconds)
    if task.get("fault"):
        from fault_proxy import fault_prerequisite
        missing = fault_prerequisite(profile)
        if missing:
            return dict(status="not_run", reason=missing, wall_seconds=0)
    folder.mkdir(parents=True)
    workspace, data, home = folder/"workspace", folder/"data", folder/"home"
    workspace.mkdir()
    template = Path(profile["home_template"]).expanduser().resolve()
    if not template.is_dir():
        return dict(status="not_run", reason="dedicated home_template directory does not exist", wall_seconds=0)
    hashes = prepare_home(template, home)
    (workspace/".ax").mkdir()
    goal = dict(enabled=True, mode="off", max_rounds=config["max_steps"], max_seconds=config["task_seconds"],
        max_tokens=config.get("task_tokens", 0), max_cost_microusd=config.get("task_cost_microusd", 0))
    goal.update(task.get("goal", {}))
    (workspace/".ax"/"goal.json").write_text(json.dumps(goal), encoding="utf-8")
    for name, content in task.get("files", {}).items():
        path = confined(workspace, name)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    env = os.environ.copy()
    env["AX_HOME"] = str(home)
    env["AX_EVENT_LOG"] = str(folder/"events.jsonl")
    argv = [str(binary), "--provider", profile["provider"], "--model", profile["model"],
        "--sandbox", "workspace", "--data-dir", str(data), "--max-steps", str(config["max_steps"]),
        "--max-tool-calls", str(config["max_tool_calls"]), "--turn-timeout-secs", str(config["task_seconds"]),
        "--tool-timeout-secs", str(min(config["task_seconds"], 60))]
    if config.get("allow_dangerous", False):
        argv += ["--allow-dangerous"]
    if task.get("context_window"):
        argv += ["--context-window", str(task["context_window"])]
    if task.get("mcp"):
        script = workspace/".ax"/"bench-service.py"
        shutil.copy2(HERE/"mcp_fixture.py", script)
        mcp = workspace/".ax"/"mcp.toml"
        mcp.write_text('[servers.bench]\ntransport="stdio"\ncommand='+json.dumps(sys.executable)+'\nargs='+json.dumps([str(script), str(workspace/".ax"/"mcp-calls.jsonl"), str(task.get("mcp_failures", 0))])+'\nenabled=true\ndescription="AX benchmark record service"\ncapabilities=["records","lookup"]\n', encoding="utf-8")
        argv += ["--mcp-config", str(mcp)]
    # Freeze ordinary benchmark delegation to bound spend and preserve accounting.
    home_config = home/"config.json"
    settings = json.loads(home_config.read_text(encoding="utf-8-sig")) if home_config.exists() else {}
    settings.update(task.get("home_config_patch", {}))
    settings["subagent"] = {"max_concurrent": 1, "max_depth": 0}
    home_config.write_text(json.dumps(settings), encoding="utf-8")
    turns = task.get("turns", [dict(prompt=task.get("prompt", ""))])
    processes = []
    current_session = None
    started = time.monotonic()
    event_paths = []
    sessions = []
    stack = ExitStack()
    try:
        if task.get("fault"):
            from fault_proxy import provider_fault
            stack.enter_context(provider_fault(profile, task, folder, env, min(config["task_seconds"], remaining_seconds)))
        for index, turn in enumerate(turns):
            event_path = folder/f"turn-{index+1}"/"events.jsonl"
            event_paths.append(event_path)
            env["AX_EVENT_LOG"] = str(event_path)
            turn_argv = list(argv)
            if turn.get("turn_timeout_seconds"):
                turn_argv[turn_argv.index("--turn-timeout-secs")+1] = str(turn["turn_timeout_seconds"])
            command = turn_argv + ["run", turn["prompt"]]
            if current_session and not turn.get("new_session", False):
                command += ["--session", current_session]
                if turn.get("resume_goal"):
                    previous = trace_metrics(event_paths[:-1])
                    if previous.get("last_goal_id"):
                        command += ["--resume-goal", previous["last_goal_id"]]
            timeout = min(config["task_seconds"]-(time.monotonic()-started), remaining_seconds-(time.monotonic()-started))
            if timeout <= 0:
                processes.append(dict(exit_code=None, stop_reason="task_timeout", wall_seconds=0))
                break
            process = launch(command, workspace, env, folder/f"turn-{index+1}", timeout, turn.get("kill_after"), task.get("fault_preserve_paths", []), data)
            processes.append(process)
            current_session = session_id(data)
            sessions.append(current_session)
            if process["exit_code"] != 0 and process["stop_reason"] != "injected_process_crash" and not turn.get("continue_on_failure"):
                break
            if process["stop_reason"] == "injected_process_crash" and not current_session:
                break
    finally:
        stack.close()
    metrics = trace_metrics(event_paths)
    from advanced_checks import session_observations
    metrics.update(session_observations(data))
    metrics["execution_seconds"] = time.monotonic()-started
    if task.get("fault"):
        from fault_proxy import fault_observations
        metrics.update(fault_observations(folder/"faults.jsonl"))
    evaluation = evaluate(task, workspace, folder/"verification", min(15, max(.001, remaining_seconds-(time.monotonic()-started))), metrics)
    final_ok = bool(processes) and processes[-1]["exit_code"] == 0 and trace_metrics(event_paths[-1:])["normal_completion"] and len(processes) == len(turns)
    if task.get("terminal") == "native_stop":
        final_ok = bool(metrics["goal_stop_reasons"]) and all(p.get("stop_reason") != "task_timeout" for p in processes)
    elif task.get("terminal") == "native_stop_or_complete":
        final_ok = final_ok or bool(metrics["goal_stop_reasons"])
    final_ok = final_ok and not metrics["trace_errors"]
    injected = metrics.get("fault_injected", False) or any(p["stop_reason"] == "injected_process_crash" for p in processes)
    preserved = {k:v for p in processes for k,v in p.get("preserved_before_fault", {}).items()}
    history_preserved = {k:v for p in processes for k,v in p.get("history_before_fault", {}).items()}
    history_loss = None
    if history_preserved:
        history_loss = False
        for relative, receipt in history_preserved.items():
            path = confined(data, relative)
            if not path.is_file():
                history_loss = True
                break
            with path.open("rb") as stream:
                if hashlib.sha256(stream.read(receipt["bytes"])).hexdigest() != receipt["sha256"]:
                    history_loss = True
                    break
    checks_ok = evaluation["passed"]
    if task.get("mcp"):
        calls_path = workspace/".ax"/"mcp-calls.jsonl"
        calls = [json.loads(s) for s in calls_path.read_text().splitlines()] if calls_path.exists() else []
        checks_ok = checks_ok and any(c.get("method") == "tools/call" and not c.get("error") for c in calls)
        checks_ok = checks_ok and metrics["mcp_executed_calls"] > 0
        if task.get("mcp_failures"):
            injected = any(c.get("error") == "injected transient tool failure" for c in calls)
            checks_ok = checks_ok and injected
    if any(t.get("kill_after") is not None for t in turns):
        checks_ok = checks_ok and injected and all(p in preserved for p in task.get("fault_preserve_paths", []))
    status = "passed" if final_ok and checks_ok else "failed"
    if not evaluation.get("scorable", True):
        status = "unscorable"
    reason = None if status == "passed" else next((p["stop_reason"] for p in processes if p.get("stop_reason") and p["stop_reason"] != "injected_process_crash"), "execution_failed" if not final_ok else "verification_failed")
    if status == "unscorable":
        reason = next(c["reason"] for c in evaluation["checks"] if not c.get("scorable", True))
    metrics["fault_injected"] = injected
    recovery_seconds = metrics.pop("recovery_seconds", None)
    result = dict(status=status, reason=reason, wall_seconds=time.monotonic()-started,
        requirement_coverage=evaluation["requirement_coverage"], processes=processes,
        evaluation=evaluation, session=current_session, sessions=sessions, template_hashes=hashes,
        score_eligible=evaluation.get("scorable", True),
        recovery_success=(status == "passed") if injected else None,
        recovery_seconds=recovery_seconds if recovery_seconds is not None else processes[-1]["wall_seconds"] if injected and len(processes)>1 else None,
        session_history_data_loss=history_loss, history_before_fault=history_preserved,
        data_loss=any(not confined(workspace,p).is_file() or digest(confined(workspace,p)) != h for p,h in preserved.items()) if preserved else None, **metrics)
    for key in ("cpu_seconds",):
        values = [p.get(key) for p in processes]
        result[key] = sum(values) if all(x is not None for x in values) else None
    for key in ("peak_rss_bytes", "rss_sampled_max_bytes"):
        result[key] = max((p[key] for p in processes if p.get(key) is not None), default=None)
    result.setdefault("network_bytes", None)
    result["artifact_sha256"] = {str(p.relative_to(folder)): digest(p) for p in folder.rglob("*") if p.is_file() and p.resolve().is_relative_to(folder.resolve()) and "home" not in p.relative_to(folder).parts and p.stat().st_size <= 100_000_000}
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--config", type=Path)
    parser.add_argument("--binary", type=Path, default=ROOT/"target"/"release"/("ax.exe" if os.name == "nt" else "ax"))
    parser.add_argument("--suite", choices=["standard", "advanced", "all"], default="standard")
    parser.add_argument("--task", action="append", help="repeat for exact task ids")
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--skip-build", action="store_true", help="use explicitly supplied binary; source/build correspondence unverified")
    args = parser.parse_args()
    if args.rounds < 3:
        parser.error("at least 3 repeats required")
    config = json.loads(args.config.read_text(encoding="utf-8-sig")) if args.config else {}
    tasks = catalog() if args.suite != "advanced" else []
    if args.suite != "standard":
        from advanced import catalog as advanced_catalog
        tasks += advanced_catalog(config.get("advanced"))
    if args.task:
        unknown = set(args.task)-{t["id"] for t in tasks}
        if unknown:
            parser.error(f"unknown tasks: {sorted(unknown)}")
        tasks = [t for t in tasks if t["id"] in args.task]
    native_needed = any(t.get("executor", "native") == "native" for t in tasks)
    if args.execute:
        validate_config(config, require_home=native_needed)
        if native_needed and not args.skip_build:
            subprocess.run(["cargo", "build", "--release", "--locked", "--offline", "-p", "cli"], cwd=ROOT, check=True)
            args.binary = ROOT/"target"/"release"/("ax.exe" if os.name == "nt" else "ax")
        if native_needed and not args.binary.is_file():
            parser.error("build current production Release AX first")
    output = (args.output or HERE/"results"/datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")).resolve()
    output.mkdir(parents=True, exist_ok=False)
    started = time.monotonic()
    result = dict(metadata=dict(schema_version=1, dataset_version=VERSION, created_utc=datetime.now(timezone.utc).isoformat(),
        execution_mode="production_cli" if args.execute else "offline_plan_no_agent_execution",
        source=source_identity(), python=sys.version, platform=platform.platform(), rounds=args.rounds,
        binary_sha256=digest(args.binary) if native_needed and args.binary.exists() else None,
        binary_source_correspondence="not_applicable_to_external_executor" if not native_needed else "unverified_external_binary" if args.skip_build else "built_from_recorded_worktree" if args.execute else "not_executed",
        config_sha256=digest(args.config) if args.config else None,
        limits={k:config[k] for k in ("allow_dangerous", "task_seconds", "max_steps", "max_tool_calls", "task_tokens", "task_cost_microusd", "global_seconds", "global_tokens", "global_cost_microusd") if k in config},
        model_profiles=[{k:p.get(k) for k in ("id", "provider", "model", "model_revision")} for p in config.get("profiles", [])]),
        runs=[], limitations=LIMITATIONS)
    if args.suite != "standard":
        from advanced import VERSION as advanced_version
        result["metadata"]["advanced_dataset_version"] = advanced_version
    used_tokens, used_cost, metered = 0, 0, True
    profiles = config.get("profiles") or [dict(id="unselected")]
    result["metadata"]["planned_attempts"] = len(profiles)*len(tasks)*args.rounds
    try:
        for profile in profiles:
            for task in tasks:
                for repeat in range(1, args.rounds+1):
                    row = dict(task=task["id"], profile=profile["id"], category=task["category"], round=repeat,
                        status="not_run", reason="model_not_selected" if not args.execute else None)
                    result["runs"].append(row)
                    reason = None
                    elapsed = time.monotonic()-started
                    if not args.execute:
                        reason = "offline_plan"
                    elif config.get("global_seconds", 0) and elapsed >= config["global_seconds"]:
                        reason = "global_time_budget"
                    elif config.get("global_tokens", 0) and used_tokens >= config["global_tokens"]:
                        reason = "global_token_budget"
                    elif config.get("global_cost_microusd", 0) and used_cost >= config["global_cost_microusd"]:
                        reason = "global_cost_budget"
                    elif not metered and (config.get("global_tokens", 0) or config.get("global_cost_microusd", 0)):
                        reason = "global_budget_unmetered_fail_closed"
                    if reason:
                        row["reason"] = reason
                    else:
                        folder = output/f"{profile['id']}--{task['id']}--{repeat}"
                        row["artifacts"] = str(folder)
                        row.update(status="running", wall_seconds=0)
                        report(result, output)
                        try:
                            row.update(execute_task(dict(task, _repeat=repeat), profile, config, args.binary.resolve(), folder,
                                config["global_seconds"]-elapsed if config.get("global_seconds") else float("inf")))
                        except KeyboardInterrupt:
                            row.update(status="interrupted", reason="controller_interrupted", wall_seconds=time.monotonic()-started-elapsed)
                            raise
                        except Exception as exc:
                            row.update(status="failed", reason=f"controller_error:{type(exc).__name__}:{exc}", wall_seconds=time.monotonic()-started-elapsed)
                            folder.mkdir(parents=True, exist_ok=True)
                            (folder/"controller-error.txt").write_text(traceback.format_exc(), encoding="utf-8")
                        used_tokens += (row.get("input_tokens") or 0)+(row.get("output_tokens") or 0)
                        used_cost += row.get("cost_microusd") or 0
                        if row["status"] not in {"not_run", "skipped"} and ((config.get("global_tokens") and (row.get("input_tokens") is None or row.get("usage_estimated") is not False)) or (config.get("global_cost_microusd") and (row.get("cost_microusd") is None or row.get("cost_estimated") is not False))):
                            metered = False
                    report(result, output)  # Durable partial report after every attempt.
    except KeyboardInterrupt:
        result["metadata"]["interrupted"] = True
        raise
    finally:
        result["metadata"]["elapsed_seconds"] = time.monotonic()-started
        report(result, output)
    print(output)
    return 1 if any(r["status"] == "failed" for r in result["runs"]) else 0


if __name__ == "__main__":
    raise SystemExit(main())
