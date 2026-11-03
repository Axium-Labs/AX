"""Concrete Advanced scenarios. Environment prerequisites are resolved at execution."""
import json

VERSION = "ax-advanced-2026.11.02.2"


def catalog(settings=None):
    settings = settings or {}
    tasks = []
    for size in (100, 1000, 5000):
        values = list(range(size))
        tasks.append(dict(id=f"long_reconcile_{size}", category="long_horizon", files={
            "events.ndjson": "\n".join('{"id":%d,"amount":%d}' % (i, i%7-3) for i in values+values[:size//2])},
            prompt="Process events.ndjson as an idempotent ledger: duplicate ids count only once. Write {count,balance} to output.json and a reusable ledger.py CLI that regenerates output.json. Verify rerunning does not change output.",
            checks=[dict(kind="json", path="output.json", expected={"count": size, "balance": sum(i%7-3 for i in values)})]))
    facts = {"port": 7319, "artifact": "cobalt", "region": "ap-7"}
    context_turns = [dict(prompt="Keep these project invariants available through the session: primary port 7319; artifact cobalt; region ap-7. Do not write files yet."),
        *[dict(prompt="Read these archived, irrelevant records for context; do not change project invariants or write files.\n"+"\n".join(f"archive_{i}_{j}: unrelated diagnostic data payload value={j}" for j in range(500))) for i in range(6)],
        dict(prompt='Write the three original project invariants to output.json as {"port":number,"artifact":string,"region":string}.')]
    tasks.append(dict(id="long_context_recall", category="long_horizon", files={}, context_window=32768,
        turns=context_turns, checks=[dict(kind="json", path="output.json", expected=facts), dict(kind="trace", field="compression_count", minimum=1)]))
    stages = settings.get("stages", 20)
    if not isinstance(stages, int) or not 2 <= stages <= 10000:
        raise ValueError("advanced.stages must be 2..10000")
    tasks.append(dict(id="long_stage_pipeline", category="long_horizon", files={"stage-0.json": json.dumps({"stage": 0, "total": 0})},
        turns=[dict(prompt=f"Continue our pipeline. Read stage-{i-1}.json, verify its stage is {i-1}, then add {i} squared to its total and write stage-{i}.json with only stage and total. Verify your output. Do not rewrite previous stage files.", new_session=(i%3 == 0)) for i in range(1, stages+1)],
        checks=[dict(kind="json", path=f"stage-{i}.json", expected={"stage": i, "total": i*(i+1)*(2*i+1)//6}) for i in range(1, stages+1)]+
            ([dict(kind="trace", field="execution_seconds", minimum=settings["minimum_seconds"])] if settings.get("minimum_seconds") else [])))
    tasks.append(dict(id="automatic_rotation", category="long_horizon", files={}, context_window=32768,
        home_config_patch={"session_rotation": {"automatic": True, "strategy": "pressure", "context_percent": 50}},
        goal={"long_project": {"enabled": True, "rotation": {"automatic": True, "strategy": "pressure", "context_percent": 50}}},
        turns=context_turns, checks=[dict(kind="json", path="output.json", expected=facts), dict(kind="trace", field="rotations", minimum=1)]))
    tasks.append(dict(id="process_crash_resume", category="fault_recovery", files={}, fault_preserve_paths=["durable.txt"],
        turns=[dict(prompt="Write durable.txt with exactly 'checkpoint-7319'. Verify it, then continue inspecting and testing the workspace for at least 60 seconds. Do not remove durable.txt.", kill_after=15),
               dict(prompt="Resume the interrupted task. Verify durable.txt and copy its exact content to answer.txt. Do not recreate durable.txt if it is missing; report the loss.", resume_goal=True)],
        checks=[dict(kind="text", path="durable.txt", expected="checkpoint-7319"), dict(kind="text", path="answer.txt", expected="checkpoint-7319")]))
    tasks.append(dict(id="mcp_transient_recovery", category="fault_recovery", mcp=True, mcp_failures=1, files={},
        prompt="Use the bench MCP record service to look up cobalt. Recover from any transient errors. Write the record value as a number in output.json.",
        checks=[dict(kind="json", path="output.json", expected=7319)]))
    for name, kind in [("api_timeout", "timeout"), ("api_rate_limit", "429"), ("api_unavailable", "503"), ("network_disconnect", "disconnect")]:
        task = dict(id=name, category="fault_recovery", fault={"kind":kind,"count":1,"delay_seconds":20}, files={},
            prompt='Use filesystem tools to write output.json containing {"verified":7319}. Verify the file and finish.',
            checks=[dict(kind="json", path="output.json", expected={"verified":7319}),
                    dict(kind="trace", field="fault_injected", equals=True), dict(kind="trace", field="recovery_transport_observed", equals=True)])
        if kind == "timeout":
            task["turns"] = [dict(prompt=task["prompt"], turn_timeout_seconds=5, continue_on_failure=True),
                dict(prompt="Resume after the unresponsive API request. "+task["prompt"], resume_goal=True)]
            task["checks"].append(dict(kind="trace", field="goal_stop_reasons", contains="timeout"))
        tasks.append(task)
    stop_prompt = "Continue inspecting seed.txt with filesystem tools and verifying the task. Do not create completion.json: the task is intentionally waiting for external completion. Keep trying until the runtime reports a stop."
    native = [
        ("native_token_budget", {"max_tokens": 1}, "Goal reported-token budget", {"require_reported_usage": True, "exercise_field":"total_tokens", "exercise_min":1}),
        ("native_cost_budget", {"max_cost_microusd": 1}, "Goal reported-cost budget", {"require_reported_cost": True, "exercise_field":"cost_microusd", "exercise_min":1}),
        ("native_time_budget", {"max_seconds": 1}, "timeout", {"exercise_field":"execution_seconds", "exercise_min":1}),
        ("native_round_budget", {"max_rounds": 2}, "step", {"exercise_field":"api_requests", "exercise_min":2}),
        ("stagnation_budget", {"max_stagnant_checks": 2, "stagnant_seconds": 0, "max_unobserved_checks": 2}, "progress is unconfirmed", {}),
    ]
    for name, goal, reason, requirements in native:
        tasks.append(dict(id=name, category="reliability_constraints", terminal="native_stop", files={"seed.txt":"stable data"},
            goal={"mode":"deterministic", "deliverables":["completion.json"], **goal}, prompt=stop_prompt,
            checks=[dict(kind="trace", field="goal_stop_reasons", contains=reason, **requirements),
                    dict(kind="trace", field="goal_states", contains="paused")]))
    tasks.append(dict(id="loop_detection", category="reliability_constraints", terminal="native_stop_or_complete", files={"seed.txt":"stable data"},
        prompt="Read seed.txt at least eight times using the exact same filesystem tool arguments and no intervening different tool. Then finish. This deliberately exercises repeated-call advice.",
        checks=[dict(kind="trace", field="loop_reminders", minimum=1, exercise_field="max_consecutive_calls", exercise_min=3)]))
    from security import security_catalog
    tasks.extend(security_catalog())
    tasks.append(dict(id="crew_cross_machine", category="distributed", executor="crew",
                      description="Run the configured dedicated Crew DAG, node faults and consistency checks", files={}, checks=[]))
    return tasks
