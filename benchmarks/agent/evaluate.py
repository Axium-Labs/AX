"""Deterministic evaluation outside the task workspace; no LLM judge."""
import ast
import json
import subprocess
import sys
import time
from pathlib import Path

# Run candidate code in a separate, bounded process. This is not an OS sandbox:
# security tasks require an externally isolated execution environment.
FUNCTION_WORKER = r'''
import copy, importlib.util, json, sys
spec = importlib.util.spec_from_file_location("candidate", sys.argv[1])
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
for value, expected in json.loads(sys.stdin.read()):
    original = copy.deepcopy(value)
    try:
        actual = m.solve(value)
        print("AX_BENCH_CASE:"+json.dumps({"passed": actual == expected and value == original}))
    except Exception as exc:
        print("AX_BENCH_CASE:"+json.dumps({"passed": False, "reason": type(exc).__name__}))
'''


def confined(root, relative):
    path = (root / relative).resolve()
    if not path.is_relative_to(root.resolve()):
        raise ValueError("path escapes workspace")
    return path


def evaluate(task, workspace, evidence, timeout=15, metrics=None):
    evidence.mkdir(parents=True, exist_ok=True)
    checks = []
    deadline = time.monotonic()+timeout
    for i, check in enumerate(task["checks"]):
        entry = {"index": i, "kind": check["kind"], "passed": False}
        try:
            path = confined(workspace, check.get("path", "solution.py"))
            if check["kind"] == "trace":
                from advanced_checks import verify_trace
                entry.update(verify_trace(check, metrics or {}))
            elif check["kind"] == "json":
                entry["passed"] = json.loads(path.read_text(encoding="utf-8")) == check["expected"]
            elif check["kind"] == "text":
                entry["passed"] = path.read_text(encoding="utf-8").strip() == check["expected"]
            elif check["kind"] == "absent":
                entry["passed"] = not path.exists()
            elif check["kind"] == "function":
                if task.get("forbidden_calls"):
                    tree = ast.parse(path.read_text(encoding="utf-8"))
                    if any(isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id in task["forbidden_calls"] for n in ast.walk(tree)):
                        raise ValueError("forbidden function call")
                proc = subprocess.run([sys.executable, "-I", "-c", FUNCTION_WORKER, str(path)],
                    input=json.dumps(check["cases"]), capture_output=True, text=True,
                    encoding="utf-8", cwd=workspace, timeout=max(.001, deadline-time.monotonic()))
                (evidence/f"check-{i}.stdout").write_text(proc.stdout, encoding="utf-8")
                (evidence/f"check-{i}.stderr").write_text(proc.stderr, encoding="utf-8")
                cases = [json.loads(line.removeprefix("AX_BENCH_CASE:")) for line in proc.stdout.splitlines() if line.startswith("AX_BENCH_CASE:")]
                entry.update(passed=proc.returncode == 0 and len(cases) == len(check["cases"]) and all(c["passed"] for c in cases), exit_code=proc.returncode,
                    cases_passed=sum(c["passed"] for c in cases), cases_total=len(check["cases"]))
            else:
                raise ValueError("unknown evaluator")
            if not entry["passed"]:
                entry["reason"] = "deterministic assertion failed"
        except (OSError, ValueError, SyntaxError, subprocess.TimeoutExpired) as exc:
            entry["reason"] = f"{type(exc).__name__}: {exc}"
        checks.append(entry)
    units = sum(c.get("cases_total", 1) for c in checks)
    passed_units = sum(c.get("cases_passed", int(c["passed"])) for c in checks)
    return {"passed": bool(checks) and all(c["passed"] for c in checks),
            "scorable": all(c.get("scorable", True) for c in checks),
            "requirement_coverage": passed_units/units if units else None,
            "coverage_unit": "declared deterministic checks, not semantic completeness", "checks": checks}
