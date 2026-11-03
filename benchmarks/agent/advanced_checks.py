"""Advanced trace assertions: outcomes must be evidenced by production events."""
import json
from pathlib import Path


def session_observations(data):
    reminders, messages, errors = 0, 0, []
    for path in sorted(data.rglob("*.jsonl")):
        try:
            with path.open(encoding="utf-8") as stream:
                for index, line in enumerate(stream):
                    try:
                        row = json.loads(line)
                        content = row.get("content", "")
                        if isinstance(content, str) and content.startswith("[ax-loop-hygiene]"):
                            reminders += 1
                        messages += 1
                    except (ValueError, AttributeError):
                        errors.append(f"{path.name}:{index+1}")
        except OSError as exc:
            errors.append(type(exc).__name__)
    return dict(loop_reminders=reminders, persisted_messages=messages, history_errors=errors)


def verify_trace(check, metrics):
    field = check["field"]
    actual = metrics.get(field)
    if check.get("require_reported_usage") and metrics.get("usage_estimated") is not False:
        return dict(passed=False, scorable=False, reason="Provider did not supply complete token usage")
    if check.get("require_reported_cost") and metrics.get("cost_estimated") is not False:
        return dict(passed=False, scorable=False, reason="Provider did not supply complete billing")
    if check.get("exercise_field") and (metrics.get(check["exercise_field"]) or 0) < check.get("exercise_min", 1):
        return dict(passed=False, scorable=False, reason="scenario trigger was not exercised by the real execution")
    if "minimum" in check:
        passed = isinstance(actual, (int, float)) and actual >= check["minimum"]
    elif "equals" in check:
        passed = actual == check["equals"]
    elif "contains" in check:
        items = actual if isinstance(actual, list) else [actual]
        passed = any(isinstance(item, str) and check["contains"] in item for item in items)
    else:
        raise ValueError("trace assertion requires minimum, equals or contains")
    return dict(passed=passed, scorable=True, field=field, actual=actual,
                reason=None if passed else "production trace assertion failed")
