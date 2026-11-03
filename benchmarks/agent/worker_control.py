"""Owned real AX worker lifecycle for dedicated benchmark hosts.

Provides portable start/crash/stop/cleanup/identity/probe hooks. It never searches
for processes by name. A retained PID birth identity and run owner marker must
match before any signal. Cleanup stops the owned process but preserves all data.
Network disconnect/reconnect requires explicit isolated-container/network hooks.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import shutil
import subprocess
import time

from node_control import IDENTITY
from resource_probe import probe


def owner_directory(path, run_id, create=False):
    path = path.resolve()
    if not IDENTITY.fullmatch(run_id) or run_id not in path.parts:
        raise ValueError("run directory must have the exact run_id as a path component")
    marker = path/"owner.json"
    if create:
        path.mkdir(parents=True, exist_ok=True)
        if not marker.exists():
            marker.write_text(json.dumps({"run_id":run_id}), encoding="utf-8")
    if not marker.exists() or json.loads(marker.read_text()).get("run_id") != run_id:
        raise ValueError("run directory ownership marker does not match")
    return path


def owned_process(root, run_id):
    state = json.loads((root/"worker.pid").read_text())
    if state.get("run_id") != run_id:
        raise ValueError("PID record run ownership mismatch")
    current = probe(state["pid"])
    if not current.get("process_identity") or current["process_identity"] != state.get("process_identity"):
        raise ValueError("PID birth identity changed; refusing to signal another process")
    return state, current


def start(root, run_id, binary=None, template=None):
    record = root/"worker.pid"
    if record.exists():
        try:
            state, current = owned_process(root, run_id)
            return dict(current, run_id=run_id, already_running=True)
        except OSError as exc:
            if exc.errno not in {2, 3, 87}:
                raise
    config_path = root/"worker.json"
    if template:
        template = Path(template).resolve()
        settings = json.loads(template.read_text(encoding="utf-8-sig"))
        if not settings.get("ax_home"):
            raise ValueError("worker template must name a dedicated benchmark ax_home")
        original_home = Path(settings["ax_home"])
        if not original_home.is_absolute():
            original_home = template.parent/original_home
        home = root/"home"
        home.mkdir(exist_ok=True)
        for name in ("config.json", "auth.json", "providers.json"):
            if (original_home/name).is_file() and not (home/name).exists():
                shutil.copy2(original_home/name, home/name)
                if os.name != "nt":
                    (home/name).chmod(0o600)
        settings["ax_home"] = str(home)
        settings["execution_root"] = str(root/"executions")
        config_path.write_text(json.dumps(settings, indent=2), encoding="utf-8")
        if os.name != "nt":
            config_path.chmod(0o600)
    elif not config_path.exists():
        raise ValueError("first start requires a dedicated worker template")
    binary_file = root/"binary.json"
    if binary:
        binary = str(Path(binary).resolve(strict=True))
        binary_file.write_text(json.dumps({"binary":binary}), encoding="utf-8")
    else:
        binary = json.loads(binary_file.read_text())["binary"]
    env = os.environ.copy()
    env["AX_EVENT_LOG"] = str(root/"events.jsonl")
    with (root/"worker.stdout").open("ab") as out, (root/"worker.stderr").open("ab") as err:
        process = subprocess.Popen([binary, "crew", "worker", str(config_path)], cwd=root, env=env,
            stdin=subprocess.DEVNULL, stdout=out, stderr=err, start_new_session=os.name != "nt",
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        try:
            snapshot = probe(process.pid)
        except OSError:
            process.kill()
            process.wait(timeout=5)
            raise RuntimeError("worker failed before its PID birth identity could be recorded")
    state = dict(run_id=run_id, pid=process.pid, process_identity=snapshot["process_identity"])
    if not state["process_identity"]:
        process.terminate()
        process.wait(timeout=5)
        raise RuntimeError("platform cannot identify process birth; refusing unmanaged worker")
    record.write_text(json.dumps(state), encoding="utf-8")
    return dict(snapshot, run_id=run_id, started=True)


def stop(root, run_id, crash=False):
    try:
        state, _ = owned_process(root, run_id)
    except OSError as exc:
        if exc.errno in {2, 3, 87}:
            return dict(run_id=run_id, already_stopped=True)
        raise
    pid = state["pid"]
    if os.name == "nt":
        # The PID was just matched against its original creation timestamp.
        command = ["taskkill", "/PID", str(pid), "/T", "/F"]
        result = subprocess.run(command, capture_output=True, timeout=15)
        if result.returncode:
            raise RuntimeError("owned worker tree termination failed")
    else:
        os.killpg(pid, signal.SIGKILL if crash else signal.SIGTERM)
        deadline = time.monotonic()+5
        while time.monotonic() < deadline:
            try:
                _, current = owned_process(root, run_id)
            except (OSError, ProcessLookupError):
                break
            time.sleep(.05)
        else:
            os.killpg(pid, signal.SIGKILL)
    return dict(run_id=run_id, stopped=True, abrupt=crash or os.name == "nt")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["start", "stop", "crash", "cleanup", "identity", "probe"])
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--run-dir", type=Path, required=True)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--worker-template", type=Path)
    args = parser.parse_args()
    root = owner_directory(args.run_dir, args.run_id, create=args.action == "start")
    if args.action == "start":
        result = start(root, args.run_id, args.binary, args.worker_template)
    elif args.action in {"stop", "crash", "cleanup"}:
        result = stop(root, args.run_id, args.action == "crash")
    else:
        _, result = owned_process(root, args.run_id)
        result["run_id"] = args.run_id
        settings = json.loads((root/"worker.json").read_text())
        result["model"] = {key:settings.get(key) for key in ("provider", "model")}
        binary = Path(json.loads((root/"binary.json").read_text())["binary"])
        result["binary_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        if args.action == "probe":
            from execution import trace_metrics
            metrics = trace_metrics(root/"events.jsonl")
            result.update({key:metrics[key] for key in ("input_tokens", "output_tokens", "api_requests", "cost_microusd", "usage_estimated", "cost_estimated")})
            result["usage_identity"] = str((root/"events.jsonl").resolve())
            result["usage_scope"] = "Goal cumulative snapshots; imported cross-node checkpoint usage may overlap"
            if metrics["input_tokens"] is not None:
                result["usage_estimated"] = True
            if metrics["cost_microusd"] is not None:
                result["cost_estimated"] = True
    print(json.dumps(result))


if __name__ == "__main__":
    main()
