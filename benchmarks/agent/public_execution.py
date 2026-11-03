"""Run public benchmark tasks through production AX and upstream deterministic tests.

Prepared local Docker images only. Planning is offline and never loads a model.
Official validation is separate from agent execution; missing scores are ungraded.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
import platform
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import time
import traceback
import uuid

from execution import launch, trace_metrics
from public import fingerprint_tree, safe_id
from report import report
from run import prepare_home, source_identity


class PrerequisiteError(ValueError):
    pass


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def validate_config(config, manifest):
    calibration = config.get("calibration", False)
    if not isinstance(calibration, bool):
        raise ValueError("calibration must be a boolean")
    if calibration and (config.get("rounds") != 1 or len(manifest.get("tasks", [])) != 1 or len(config.get("profiles", [])) != 1):
        raise ValueError("cost calibration requires exactly one task, one profile and one repetition")
    if not calibration and config.get("rounds", 3) < 3:
        raise ValueError("at least three repeats required")
    if not config.get("profiles") or not config.get("environments"):
        raise ValueError("profiles and prepared environments required")
    for key in ("task_seconds", "verifier_seconds", "max_steps", "max_tool_calls", "memory_mb", "pids_limit"):
        if not isinstance(config.get(key), int) or config[key] <= 0:
            raise ValueError(f"{key} must be a positive integer")
    if isinstance(config.get("cpus"), bool) or not isinstance(config.get("cpus"), (int, float)) or not math.isfinite(config["cpus"]) or config["cpus"] <= 0:
        raise ValueError("positive finite cpus required")
    if not isinstance(config.get("allow_dangerous", False), bool):
        raise ValueError("allow_dangerous must be an explicit boolean")
    for key in ("global_seconds", "global_tokens", "global_cost_microusd", "task_tokens", "task_cost_microusd"):
        if not isinstance(config.get(key, 0), int) or config.get(key, 0) < 0:
            raise ValueError(f"invalid {key}")
    for key in ("container_uid", "container_gid"):
        value = config.get(key, 0)
        if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value < 2**31:
            raise ValueError("container uid/gid must be nonnegative integers")
    if config.get("nested_userns_systempaths", False) and (not isinstance(config["nested_userns_systempaths"], bool) or config.get("container_uid", 0) == 0 or not config.get("seccomp_profile")):
        raise ValueError("nested userns setup requires a non-root user and pinned default-deny seccomp profile")
    allowed_verifier_caps = {"CHOWN", "DAC_OVERRIDE", "FOWNER", "SETUID", "SETGID"}
    caps = config.get("verifier_capabilities", [])
    if not isinstance(caps, list) or any(c not in allowed_verifier_caps for c in caps):
        raise ValueError("verifier capabilities must be explicitly selected from limited package-install permissions")
    if not manifest.get("dataset_revision"):
        raise ValueError("dataset revision required")
    ids = [safe_id(t["id"]) for t in manifest["tasks"]]
    if len(set(ids)) != len(ids):
        raise ValueError("duplicate task ids")
    profiles = [safe_id(p["id"]) for p in config["profiles"]]
    if len(set(profiles)) != len(profiles):
        raise ValueError("duplicate profile ids")
    for profile in config["profiles"]:
        if not all(profile.get(k) for k in ("provider", "model", "home_template")):
            raise ValueError("profile requires provider, model and dedicated home_template")
        if profile.get("reasoning_effort") not in {None, "low", "medium", "high", "xhigh", "max"}:
            raise ValueError("unsupported reasoning_effort")
    for task in manifest["tasks"]:
        env = config["environments"].get(task["id"])
        if not env:
            raise ValueError(f"missing prepared environment for {task['id']}")
        if not re.fullmatch(r"(?:[^\s]+@)?sha256:[a-f0-9]{64}", env.get("image", "")):
            raise ValueError("agent image must be pinned by SHA256 digest, never a mutable tag")
        path = PurePosixPath(env.get("workspace", ""))
        if not path.is_absolute() or ".." in path.parts or str(path) == "/" or str(path).startswith("/axbench"):
            raise ValueError("dedicated absolute container workspace required")
        threshold = env.get("success_threshold", 1.0)
        if isinstance(threshold, bool) or not isinstance(threshold, (int, float)) or not math.isfinite(threshold):
            raise ValueError("reward threshold must be finite")
        if task.get("format") not in {"swe-bench", "terminal-v1", "terminal-v2"}:
            raise ValueError("unsupported public task format")


def validate_sources(task):
    if task["format"].startswith("terminal"):
        root = Path(task["task_directory"]).resolve()
        if not root.is_dir() or fingerprint_tree(root) != task["source_files"]:
            raise PrerequisiteError("pinned Terminal task files changed or unavailable")
        if not task.get("prompt") or task.get("missing"):
            raise PrerequisiteError("incomplete Terminal task layout")


def reject_workspace_volumes(inspection, workspace):
    root = PurePosixPath(workspace)
    for value in (inspection.get("Config", {}).get("Volumes") or {}):
        volume = PurePosixPath(value)
        if root.is_relative_to(volume) or volume.is_relative_to(root) or PurePosixPath("/axbench").is_relative_to(volume) or volume.is_relative_to(PurePosixPath("/axbench")):
            raise PrerequisiteError("image VOLUME overlaps candidate or AX data; Docker commit would lose task state")


def pinned_seccomp(config):
    name = config.get("seccomp_profile")
    if not name:
        return None
    path = Path(name).resolve()
    if not path.is_file() or digest(path) != config.get("seccomp_sha256"):
        raise PrerequisiteError("vetted Docker seccomp profile missing or hash changed")
    policy = json.loads(path.read_text(encoding="utf-8"))
    if policy.get("defaultAction") not in {"SCMP_ACT_ERRNO", "SCMP_ACT_KILL", "SCMP_ACT_KILL_PROCESS"}:
        raise PrerequisiteError("a default-allow/unconfined seccomp policy is not accepted")
    return path


def verify_checkout(spec):
    root = Path(spec["source_dir"]).resolve()
    commit = spec.get("commit", "")
    if not re.fullmatch(r"[a-fA-F0-9]{40}", commit):
        raise PrerequisiteError("full official evaluator Git commit required")
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    dirty = subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=all"], cwd=root, text=True).strip()
    if head != commit or dirty:
        raise PrerequisiteError("official evaluator must be an unchanged pinned checkout")
    return root


def parse_swe_report(path, instance):
    if not path.is_file():
        return None
    try:
        result = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    if not isinstance(result, dict) or instance not in result:
        return None
    if not isinstance(result[instance], dict):
        return None
    resolved = result[instance].get("resolved")
    if not isinstance(resolved, bool):
        return None
    return {"passed": resolved, "score": float(resolved), "official_report": result[instance], "report_sha256": digest(path)}


def parse_reward(folder, reward_key=None, threshold=1.0):
    text_path, json_path = folder/"reward.txt", folder/"reward.json"
    try:
        if text_path.is_file():
            value = float(text_path.read_text(encoding="utf-8").strip())
            source = text_path
        elif json_path.is_file():
            if not reward_key:
                return None  # Multi-dimensional reward needs an explicit declared key.
            payload = json.loads(json_path.read_text(encoding="utf-8"))
            if not isinstance(payload, dict) or reward_key not in payload or isinstance(payload[reward_key], bool):
                return None
            value, source = float(payload[reward_key]), json_path
        else:
            return None
    except (OSError, ValueError, TypeError):
        return None
    if not math.isfinite(value):
        return None
    return dict(passed=value >= threshold, score=value, report_sha256=digest(source), reward_source=source.name)


def swe_dataset_row(task, config):
    spec = config.get("swe_evaluator")
    if not spec:
        raise PrerequisiteError("official SWE evaluator not configured")
    verify_checkout(spec)
    original = Path(spec["dataset_file"]).resolve()
    if digest(original) != config["manifest_dataset_sha256"]:
        raise PrerequisiteError("official SWE dataset differs from imported dataset")
    def read_rows(path):
        text = path.read_text(encoding="utf-8-sig")
        return json.loads(text) if path.suffix == ".json" else [json.loads(line) for line in text.splitlines() if line.strip()]
    originals = [r for r in read_rows(original) if r["instance_id"] == task["id"]]
    if len(originals) != 1:
        raise PrerequisiteError("official dataset instance missing or duplicated")
    prepared = Path(spec.get("prepared_dataset_file", original)).resolve()
    if prepared != original and digest(prepared) != spec.get("prepared_dataset_sha256"):
        raise PrerequisiteError("prepared official task dataset hash mismatch")
    selected = [r for r in read_rows(prepared) if r["instance_id"] == task["id"]]
    if len(selected) != 1:
        raise PrerequisiteError("prepared official dataset instance missing or duplicated")
    selected = dict(selected[0])
    for key in ("repo", "base_commit", "test_patch", "FAIL_TO_PASS", "PASS_TO_PASS"):
        if key in originals[0] and selected.get(key) != originals[0][key]:
            raise PrerequisiteError(f"prepared official task changed {key}")
    if not all(key in selected for key in ("eval_script", "log_parser", "eval_type", "version", "FAIL_TO_PASS", "PASS_TO_PASS")):
        raise PrerequisiteError("prepare official SWE task eval_script/parser metadata before execution")
    image = config["environments"][task["id"]].get("evaluation_image", selected.get("image", ""))
    if not re.fullmatch(r"(?:[^\s]+@)?sha256:[a-f0-9]{64}", image):
        raise PrerequisiteError("official evaluation image must be pinned by digest")
    selected["image"] = image
    return selected


def budget_reason(config, rows, elapsed):
    if config.get("global_seconds") and elapsed >= config["global_seconds"]:
        return "global_time_budget"
    executed = [r for r in rows if r.get("agent_executed")]
    for limit, keys, flag in [("global_tokens", ("input_tokens", "output_tokens"), "usage_estimated"),
                              ("global_cost_microusd", ("cost_microusd",), "cost_estimated")]:
        if not config.get(limit):
            continue
        if any(r.get(flag) is not False or any(r.get(k) is None for k in keys) for r in executed):
            return "global_budget_unmetered_fail_closed"
        if sum(sum(r[k] for k in keys) for r in executed) >= config[limit]:
            return limit
    return None


class Trial:
    """Owns exactly this trial's containers and temporary snapshot image."""
    def __init__(self, config, folder, deadline):
        self.config, self.folder, self.deadline = config, folder, deadline
        self.docker = config.get("docker", "docker")
        self.counter = 0
        self.containers, self.snapshots = [], []
        self.upload_owners = {}
        self.cleanup_errors = []

    def command(self, args, label, *, check=True, timeout=None, cwd=None, env=None):
        self.counter += 1
        folder = self.folder/f"command-{self.counter:03}-{label}"
        left = self.deadline-time.monotonic()
        if left <= 0:
            raise TimeoutError("public task deadline exhausted")
        result = launch(args, cwd or self.folder, env or os.environ.copy(), folder,
                        min(left, timeout if timeout is not None else left))
        stdout = folder/"stdout.txt"
        if stdout.stat().st_size > self.config.get("max_capture_bytes", 20_000_000):
            raise ValueError("controller capture limit exceeded; full evidence retained")
        output = stdout.read_text(encoding="utf-8", errors="replace")
        if result["stop_reason"]:
            raise TimeoutError(result["stop_reason"])
        if check and result["exit_code"]:
            raise RuntimeError(f"{label} exited {result['exit_code']}; see {folder.name}")
        return output, result

    def docker_command(self, args, label, **kwargs):
        return self.command([self.docker, *args], label, **kwargs)

    def create(self, image, network, label):
        name = "axbench-public-"+uuid.uuid4().hex
        owner = (0, 0) if label == "verifier" else (self.config.get("container_uid", 0), self.config.get("container_gid", self.config.get("container_uid", 0)))
        self.upload_owners[name] = owner
        args = ["create", "--pull=never", "--name", name, "--label", "axbench.public=true",
                "--network", network, "--cpus", str(self.config["cpus"]), "--memory", f"{self.config['memory_mb']}m",
                "--pids-limit", str(self.config["pids_limit"]), "--security-opt", "no-new-privileges:true",
                "--cap-drop", "ALL", "--user", f"{owner[0]}:{owner[1]}"]
        if label == "verifier":
            args += ["--env", "HOME=/root"]
            for capability in self.config.get("verifier_capabilities", []):
                args += ["--cap-add", capability]
        elif self.config.get("nested_userns_systempaths", False):
            # bwrap disables further user namespaces through a sysctl scoped to
            # its newly created user namespace. Docker's blanket /proc/sys
            # read-only mount prevents that setup. This explicit non-root mode
            # retains cap-drop, no-new-privileges, default-deny seccomp and the
            # actual AX sandbox preflight; it never uses --privileged.
            args += ["--security-opt", "systempaths=unconfined"]
        seccomp = pinned_seccomp(self.config)
        if seccomp:
            args += ["--security-opt", "seccomp="+str(seccomp)]
        args += ["--entrypoint", "/bin/sh", image, "-c", "while :; do sleep 3600; done"]
        self.containers.append(name)  # Own the name even if create is interrupted.
        self.docker_command(args, label+"-create")
        self.docker_command(["start", name], label+"-start")
        return name

    def cp_to(self, source, container, destination):
        # Docker/containerd may preserve host UID on copied files even without
        # --archive. Normalize trusted uploads before inference; never grant
        # FOWNER/CHOWN capabilities to the candidate just to repair setup.
        path = Path(source).resolve()
        target = PurePosixPath(destination)
        if not target.is_absolute() or ".." in target.parts:
            raise ValueError("absolute container upload target required")
        archive = self.folder/f"private-upload-{self.counter+1}.tar"
        def normalize(info):
            if not info.isfile() and not info.isdir():
                raise ValueError("upload symlinks/special files are forbidden")
            info.uid, info.gid = self.upload_owners.get(container, (0, 0))
            info.uname = info.gname = ""
            return info
        try:
            with archive.open("xb") as stream:
                os.chmod(archive, 0o600)
                with tarfile.open(fileobj=stream, mode="w") as tar:
                    tar.add(path, arcname=target.name, filter=normalize)
            self.counter += 1
            folder = self.folder/f"command-{self.counter:03}-copy-in"
            folder.mkdir()
            with archive.open("rb") as stream, (folder/"stdout.txt").open("wb") as out, (folder/"stderr.txt").open("wb") as err:
                process = subprocess.run([self.docker, "cp", "-", f"{container}:{target.parent}"], stdin=stream,
                    stdout=out, stderr=err, timeout=max(.001, self.deadline-time.monotonic()))
            if process.returncode:
                raise RuntimeError("normalized container upload failed; inspect retained copy logs")
        finally:
            archive.unlink(missing_ok=True)

    def cp_from(self, container, source, destination, optional=False):
        destination.parent.mkdir(parents=True, exist_ok=True)
        return self.docker_command(["cp", f"{container}:{source}", str(destination)], "copy-evidence", check=not optional)

    def cleanup(self, retained=None):
        # Cleanup has its own small bound even after the experiment deadline.
        for name in reversed(self.containers):
            try:
                exists = subprocess.run([self.docker, "inspect", "--format", "{{.Id}}", name], capture_output=True, timeout=5)
                if exists.returncode:
                    continue  # Upstream SWE evaluation normally cleans its own container.
                if name == retained:
                    removed = subprocess.run([self.docker, "exec", name, "/bin/rm", "-rf", "--", "/axbench/home"], capture_output=True, timeout=5)
                    if removed.returncode:
                        # Failed credential removal must never leave an authenticated
                        # stopped candidate behind for later accidental export.
                        retained = None
                args = [self.docker, "stop", "--time", "1", name] if name == retained else [self.docker, "rm", "--force", name]
                proc = subprocess.run(args, capture_output=True, timeout=15)
                if proc.returncode:
                    self.cleanup_errors.append({"container": name, "exit_code": proc.returncode})
                    if name == retained:
                        retained = None
            except (OSError, subprocess.TimeoutExpired) as exc:
                self.cleanup_errors.append({"container": name, "error": type(exc).__name__})
                if name == retained:
                    retained = None
                try:
                    subprocess.run([self.docker, "rm", "--force", name], capture_output=True, timeout=10)
                except (OSError, subprocess.TimeoutExpired):
                    self.cleanup_errors.append({"container": name, "error": "force_remove_failed"})
        for image in reversed(self.snapshots):
            try:
                proc = subprocess.run([self.docker, "image", "rm", image], capture_output=True, timeout=15)
                if proc.returncode:
                    self.cleanup_errors.append({"snapshot": image, "exit_code": proc.returncode})
            except (OSError, subprocess.TimeoutExpired) as exc:
                self.cleanup_errors.append({"snapshot": image, "error": type(exc).__name__})
        return retained


SWE_LAUNCHER = r'''import json, os, runpy, sys
import docker.models.containers
import docker.models.images
limits = json.loads(os.environ["AX_BENCH_OFFICIAL_LIMITS"])
original = docker.models.containers.ContainerCollection.create
def bounded(self, *args, **kwargs):
    if kwargs.get("name") != limits["name"]:
        raise RuntimeError("official evaluator requested a container outside this trial")
    kwargs.update(mem_limit=limits["memory_bytes"], nano_cpus=limits["nano_cpus"], pids_limit=limits["pids_limit"], network=limits["network"])
    return original(self, *args, **kwargs)
def no_prepare(*args, **kwargs):
    raise RuntimeError("all official evaluator images must already be prepared locally")
docker.models.containers.ContainerCollection.create = bounded
docker.models.images.ImageCollection.pull = no_prepare
docker.models.images.ImageCollection.build = no_prepare
sys.argv = ["swebench.harness.run_evaluation", *sys.argv[1:]]
runpy.run_module("swebench.harness.run_evaluation", run_name="__main__")
'''


def official_swe(trial, task, config, patch):
    spec = config.get("swe_evaluator")
    if not spec:
        return None, "official_swe_evaluator_not_configured"
    source = verify_checkout(spec)
    selected = swe_dataset_row(task, config)
    run_id = "axbench-"+uuid.uuid4().hex
    model_name = "axbench-production-ax"
    verification = trial.folder/"verification"
    verification.mkdir()
    predictions = verification/"predictions.jsonl"
    predictions.write_text(json.dumps(dict(instance_id=task["id"], model_name_or_path=model_name, model_patch=patch))+"\n", encoding="utf-8")
    # Upstream accepts dataset paths; controller-only gold/test fields never enter AX.
    official_data = verification/"dataset.json"
    official_data.write_text(json.dumps([selected]), encoding="utf-8")
    env = os.environ.copy()
    env["PYTHONPATH"] = str(source)
    env["AX_BENCH_OFFICIAL_LIMITS"] = json.dumps(dict(name=f"sweb.eval.{task['id'].lower()}.{run_id}",
        memory_bytes=config["memory_mb"]*1024*1024, nano_cpus=int(config["cpus"]*1_000_000_000),
        pids_limit=config["pids_limit"], network=config.get("verifier_network", "none")))
    python = str(Path(spec.get("python", os.sys.executable)).resolve())
    argv = [python, "-c", SWE_LAUNCHER, "--dataset_name", str(official_data),
            "--predictions_path", str(predictions), "--instance_ids", task["id"], "--run_id", run_id,
            "--max_workers", "1", "--timeout", str(config["verifier_seconds"])]
    try:
        _, result = trial.command(argv, "official-swe", check=False, timeout=config["verifier_seconds"], cwd=verification, env=env)
    finally:
        # This unique name is the documented upstream container name; never touch
        # unrelated evaluation containers if the official process was interrupted.
        name = f"sweb.eval.{task['id'].lower()}.{run_id}"
        if name not in trial.containers:
            trial.containers.append(name)
    path = verification/"logs"/"evaluation"/run_id/model_name/task["id"]/"report.json"
    parsed = parse_swe_report(path, task["id"])
    if result["exit_code"] != 0:
        return None, "official_evaluator_process_failed"
    if parsed is None:
        return None, "official_instance_score_missing"
    parsed.update(evaluator_commit=spec["commit"], run_id=run_id, predictions_sha256=digest(predictions), dataset_sha256=digest(official_data), evaluation_image=selected["image"])
    return parsed, None


LEGACY_PARSER = '''import json, pathlib, sys
from terminal_bench.parsers.parser_factory import ParserFactory, ParserName
parser = ParserFactory.get_parser(ParserName(sys.argv[1]))
results = parser.parse(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8", errors="replace"))
pathlib.Path(sys.argv[3]).write_text(json.dumps({k:v.value for k,v in results.items()}))
'''


def official_terminal(trial, task, config, agent, image):
    # Snapshot only after AX stops and credentials are removed. The verifier runs
    # in a NEW container: candidate background processes cannot observe hidden tests.
    snapshot, _ = trial.docker_command(["commit", agent], "freeze-candidate")
    snapshot = snapshot.strip()
    if not re.fullmatch(r"sha256:[a-f0-9]{64}", snapshot):
        raise ValueError("Docker returned invalid snapshot identity")
    trial.snapshots.append(snapshot)
    trial.docker_command(["stop", "--time", "1", agent], "stop-agent-before-verifier")
    verifier = trial.create(snapshot, config.get("verifier_network", "none"), "verifier")
    root = Path(task["task_directory"])
    trial.docker_command(["exec", verifier, "/bin/rm", "-rf", "--", "/tests", "/logs/verifier"], "clear-stale-verifier-state")
    trial.docker_command(["exec", verifier, "/bin/mkdir", "-p", "/tests", "/logs/verifier"], "prepare-verifier")
    trial.cp_to(str(root/task["tests_source"])+"/.", verifier, "/tests/")
    if task["format"] == "terminal-v1":
        trial.cp_to(root/"run-tests.sh", verifier, "/axbench/run-tests.sh")
    _, result = trial.docker_command(["exec", "--env", "TEST_DIR=/tests", "--workdir", config["environments"][task["id"]]["workspace"], verifier,
        "/bin/bash", task["verifier_script"]], "official-terminal", check=False, timeout=config["verifier_seconds"])
    verification = trial.folder/"verification"
    verification.mkdir(exist_ok=True)
    if task["format"] == "terminal-v2":
        trial.cp_from(verifier, "/logs/verifier/.", verification, optional=True)
        required = config["environments"][task["id"]].get("required_verifier_files", [])
        if any(not (verification/name).is_file() for name in required):
            return None, "required_verifier_evidence_missing"
        parsed = parse_reward(verification, config["environments"][task["id"]].get("reward_key"), config["environments"][task["id"]].get("success_threshold", 1.0))
        if parsed and parsed["passed"] and result["exit_code"]:
            return None, "verifier_error_despite_success_reward"
        return parsed, None if parsed else "official_reward_missing"
    spec = config.get("terminal_evaluator")
    if not spec:
        return None, "official_terminal_parser_not_configured"
    source = verify_checkout(spec)
    # Feed the unchanged upstream parser the actual verifier output, never a
    # controller regex that guesses success from a process exit code.
    # cp_to/cp_from can change the counter; locate the unique verifier command.
    output = next(trial.folder.glob("command-*-official-terminal/stdout.txt"))
    parsed_path = verification/"parsed-tests.json"
    env = os.environ.copy()
    env["PYTHONPATH"] = str(source)
    _, parser_result = trial.command([spec.get("python", os.sys.executable), "-c", LEGACY_PARSER,
        task.get("parser_name", "pytest"), str(output), str(parsed_path)], "official-terminal-parser", check=False,
        timeout=config["verifier_seconds"], env=env)
    if parser_result["exit_code"] or not parsed_path.is_file():
        return None, "official_parser_failed"
    cases = json.loads(parsed_path.read_text(encoding="utf-8"))
    if not cases or any(v not in {"passed", "failed"} for v in cases.values()):
        return None, "official_test_cases_missing"
    passed = all(v == "passed" for v in cases.values())
    if passed and result["exit_code"]:
        return None, "verifier_error_despite_passed_cases"
    return dict(passed=passed, score=sum(v == "passed" for v in cases.values())/len(cases),
                test_cases=cases, report_sha256=digest(parsed_path), evaluator_commit=spec["commit"]), None


def execute_public(task, profile, config, binary, folder, remaining_seconds):
    started = time.monotonic()
    folder.mkdir(parents=True)
    trial = Trial(config, folder, started+min(remaining_seconds, config["task_seconds"]+config["verifier_seconds"]+120))
    row = dict(status="not_run", reason=None, agent_executed=False, official_score=None)
    agent = None
    try:
        validate_sources(task)
        if not shutil.which(config.get("docker", "docker")):
            raise PrerequisiteError("Docker CLI unavailable")
        if not binary.is_file():
            raise PrerequisiteError("production Linux AX binary unavailable")
        probe = Path(config.get("sandbox_probe_binary", "missing-linux-sandbox-probe")).resolve()
        if not probe.is_file():
            raise PrerequisiteError("build the production sandbox preflight probe before model execution")
        if not Path(profile["home_template"]).is_dir():
            raise PrerequisiteError("dedicated AX Home template unavailable")
        env_config = config["environments"][task["id"]]
        inspection, _ = trial.docker_command(["image", "inspect", env_config["image"]], "inspect-prepared-image")
        inspected = json.loads(inspection)[0]
        reject_workspace_volumes(inspected, env_config["workspace"])
        pinned_seccomp(config)
        row["image"] = {k:inspected.get(k) for k in ("Id", "RepoDigests", "Os", "Architecture")}
        if task["format"] == "swe-bench":
            selected = swe_dataset_row(task, config)
            official_inspection, _ = trial.docker_command(["image", "inspect", selected["image"]], "inspect-official-evaluation-image")
            row["evaluation_image"] = {k:json.loads(official_inspection)[0].get(k) for k in ("Id", "RepoDigests", "Os", "Architecture")}
            evaluator = config["swe_evaluator"]
            eval_env = os.environ.copy()
            eval_env["PYTHONPATH"] = str(verify_checkout(evaluator))
            trial.command([evaluator.get("python", os.sys.executable), "-m", "swebench.harness.run_evaluation", "--help"],
                "official-swe-dependency-preflight", timeout=30, env=eval_env)
        elif task["format"] == "terminal-v1":
            if not config.get("terminal_evaluator"):
                raise PrerequisiteError("pinned official Terminal parser checkout required")
            evaluator = config["terminal_evaluator"]
            eval_env = os.environ.copy()
            eval_env["PYTHONPATH"] = str(verify_checkout(evaluator))
            trial.command([evaluator.get("python", os.sys.executable), "-c", "from terminal_bench.parsers.parser_factory import ParserFactory"],
                "official-terminal-dependency-preflight", timeout=30, env=eval_env)
        agent = trial.create(env_config["image"], config.get("agent_network", "bridge"), "agent")
        trial.docker_command(["exec", agent, "/bin/mkdir", "-p", "/axbench", "/axbench/data", env_config["workspace"]+"/.ax"], "prepare-agent")
        for hidden in env_config.get("forbidden_agent_paths", ["/tests", "/solution", "/solution.sh", "/run-tests.sh"]) if task["format"].startswith("terminal") else []:
            _, result = trial.docker_command(["exec", agent, "/bin/sh", "-c", 'test ! -e "$1"', "axbench", hidden], "check-heldout-not-baked", check=False)
            if result["exit_code"]:
                raise PrerequisiteError("prepared agent image exposes held-out tests/reference solution")
        if task["format"] == "swe-bench":
            head, _ = trial.docker_command(["exec", "--workdir", env_config["workspace"], agent, "git", "rev-parse", "HEAD"], "check-base-commit")
            if head.strip() != task["base_commit"]:
                raise PrerequisiteError("prepared repository HEAD differs from dataset base_commit")
        home = folder/"private-home"
        row["home_template_hashes"] = prepare_home(Path(profile["home_template"]).resolve(), home)
        settings = json.loads((home/"config.json").read_text(encoding="utf-8-sig")) if (home/"config.json").exists() else {}
        settings["subagent"] = {"max_concurrent": 1, "max_depth": 0}
        (home/"config.json").write_text(json.dumps(settings), encoding="utf-8")
        goal = folder/"goal.json"
        goal.write_text(json.dumps(dict(enabled=True, mode="off", max_rounds=config["max_steps"], max_seconds=config["task_seconds"],
            max_tokens=config.get("task_tokens", 0), max_cost_microusd=config.get("task_cost_microusd", 0))), encoding="utf-8")
        prompt = folder/"prompt.txt"
        prompt.write_text(task["prompt"], encoding="utf-8")
        trial.cp_to(binary, agent, "/axbench/ax")
        trial.cp_to(probe, agent, "/axbench/sandbox-probe")
        trial.cp_to(home, agent, "/axbench/home")
        trial.cp_to(prompt, agent, "/axbench/prompt.txt")
        trial.cp_to(goal, agent, env_config["workspace"]+"/.ax/goal.json")
        trial.docker_command(["exec", agent, "/bin/chmod", "+x", "/axbench/ax"], "make-ax-executable")
        trial.docker_command(["exec", agent, "/bin/chmod", "+x", "/axbench/sandbox-probe"], "make-sandbox-probe-executable")
        preflight, preflight_process = trial.docker_command(["exec", "--workdir", env_config["workspace"], "--env", "AX_HOME=/axbench/probe-home",
            agent, "/axbench/sandbox-probe", env_config["workspace"]], "production-sandbox-preflight", check=False, timeout=30)
        if preflight_process["exit_code"]:
            raise PrerequisiteError("production sandbox backend cannot prepare/execute in this container; no model requested")
        try:
            probe_result = json.loads(preflight.strip().splitlines()[-1])
        except (ValueError, IndexError):
            raise PrerequisiteError("production sandbox probe returned no valid receipt")
        if probe_result != {"probe":"production_sandbox_prepare_and_execute", "mode":"workspace", "passed":True}:
            raise PrerequisiteError("production sandbox probe receipt invalid")
        row["sandbox_preflight"] = dict(probe_result, binary_sha256=digest(probe))
        version, _ = trial.docker_command(["exec", agent, "/axbench/ax", "--version"], "production-ax-version")
        if not version.strip().startswith("ax "):
            raise PrerequisiteError("configured binary does not identify as AX")
        row["binary_version"] = version.strip()
        argv = ["exec", "--workdir", env_config["workspace"], "--env", "AX_HOME=/axbench/home", "--env", "AX_EVENT_LOG=/axbench/events.jsonl"]
        for name in profile.get("env_from", []):
            if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name) or name not in os.environ:
                raise PrerequisiteError("required provider environment variable unavailable")
            argv += ["--env", name]  # Docker reads value from environment; no secrets in argv/log.
        argv += [agent, "/bin/sh", "-c", 'exec "$@" run "$(cat /axbench/prompt.txt)"', "axbench",
            "/axbench/ax", "--provider", profile["provider"], "--model", profile["model"], "--sandbox", "workspace", "--data-dir", "/axbench/data",
            "--max-steps", str(config["max_steps"]), "--max-tool-calls", str(config["max_tool_calls"]),
            "--turn-timeout-secs", str(config["task_seconds"]), "--tool-timeout-secs", str(min(config["task_seconds"], 60))]
        if config.get("allow_dangerous", False):
            argv += ["--allow-dangerous"]
        if profile.get("reasoning_effort"):
            argv += ["--reasoning-effort", profile["reasoning_effort"]]
        row["agent_executed"] = True
        try:
            _, process = trial.docker_command(argv, "production-ax", check=False, timeout=config["task_seconds"])
            row["agent_process"] = process
        finally:
            # Never copy credentials into the published evidence or verifier image.
            trial.docker_command(["exec", agent, "/bin/rm", "-rf", "--", "/axbench/home"], "remove-container-credentials")
            trial.cp_from(agent, "/axbench/events.jsonl", folder/"events.jsonl", optional=True)
            trial.cp_from(agent, "/axbench/data", folder/"session-data", optional=True)
        row.update(trace_metrics(folder/"events.jsonl"))
        if process["exit_code"] != 0 or row["trace_errors"] or not row["normal_completion"]:
            row.update(status="failed", reason="agent_execution_failed")
        else:
            if task["format"] == "swe-bench":
                # Only this disposable candidate index changes; include newly created
                # files, but exclude controller Goal configuration from the patch.
                trial.docker_command(["exec", "--workdir", env_config["workspace"], agent, "git", "add", "--intent-to-add", "--", ".", ":(exclude).ax"], "include-new-candidate-files")
                patch, _ = trial.docker_command(["exec", "--workdir", env_config["workspace"], agent, "git", "diff", "--binary", task["base_commit"], "--", ".", ":(exclude).ax"], "candidate-patch")
                (folder/"candidate.patch").write_text(patch, encoding="utf-8")
                trial.docker_command(["stop", "--time", "1", agent], "stop-agent-before-official-eval")
                parsed, reason = official_swe(trial, task, config, patch)
            else:
                parsed, reason = official_terminal(trial, task, config, agent, env_config["image"])
            if parsed is None:
                row.update(status="ungraded", reason=reason, official_score=None)
            else:
                row.update(status="passed" if parsed["passed"] else "failed", reason=None if parsed["passed"] else "official_tests_failed",
                    official_score=parsed["score"], evaluation=parsed, requirement_coverage=parsed["score"])
    except PrerequisiteError as exc:
        row.update(status="ungraded" if row["agent_executed"] else "not_run", reason=str(exc))
    except KeyboardInterrupt:
        row.update(status="interrupted", reason="controller_interrupted")
        raise
    except Exception as exc:
        row.update(status="ungraded" if row["agent_executed"] and "agent_process" in row else "failed" if row["agent_executed"] else "not_run", reason=f"controller_error:{type(exc).__name__}")
        (folder/"controller-error.txt").write_text(traceback.format_exc(), encoding="utf-8")
    finally:
        retained = agent if row["status"] in {"failed", "ungraded", "interrupted"} and config.get("retain_failed_containers", True) else None
        retained = trial.cleanup(retained)
        row.update(wall_seconds=max(.000001, time.monotonic()-started), cleanup_errors=trial.cleanup_errors,
                   retained_container=retained, cpu_seconds=None, peak_rss_bytes=None, network_bytes=None,
                   resource_scope="Docker client telemetry is not AX telemetry; remote container metrics unavailable unless collected separately")
        # Private credential template retained locally for crash recovery, never in reports.
        (folder/"trial.json").write_text(json.dumps(row, indent=2), encoding="utf-8")
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    config = json.loads(args.config.read_text(encoding="utf-8-sig"))
    manifest_path = Path(config["manifest"]).resolve()
    manifest = json.loads(manifest_path.read_text(encoding="utf-8-sig"))
    validate_config(config, manifest)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    binary = Path(config.get("ax_binary", "missing-linux-ax")).resolve()
    config["manifest_dataset_sha256"] = manifest.get("dataset_sha256")
    started = time.monotonic()
    result = dict(metadata=dict(execution_mode="production_ax_public_containers" if args.execute else "offline_plan",
        measurement_scope="single_task_cost_calibration" if config.get("calibration") else "repeated_capability_benchmark",
        platform=platform.platform(), python=os.sys.version,
        created_utc=datetime.now(timezone.utc).isoformat(), config_sha256=digest(args.config), manifest_sha256=digest(manifest_path),
        dataset_revision=manifest["dataset_revision"], dataset_sha256=manifest.get("dataset_sha256"), source=source_identity(),
        binary_sha256=digest(binary) if binary.is_file() else None,
        binary_source_correspondence="externally_supplied_linux_binary_hash_only", ax_binary_commit=config.get("ax_binary_commit"),
        sandbox_probe_sha256=digest(config["sandbox_probe_binary"]) if config.get("sandbox_probe_binary") and Path(config["sandbox_probe_binary"]).is_file() else None,
        seccomp_sha256=config.get("seccomp_sha256"),
        container_uid=config.get("container_uid", 0), container_gid=config.get("container_gid", config.get("container_uid", 0)),
        nested_userns_systempaths=config.get("nested_userns_systempaths", False), agent_network=config.get("agent_network", "bridge"),
        model_profiles=[{k:p.get(k) for k in ("id", "provider", "model", "model_revision", "reasoning_effort")} for p in config["profiles"]]),
        runs=[], limitations=["Public task fixtures and official verifiers are used directly; no alternate Agent/harness comparison is run.",
            "Official score absence is ungraded, never an assumed zero/pass. Runtime failure is retained separately.",
            "Images, source checkouts and models are external prepared prerequisites. No image build/pull/cloud provisioning is performed by this driver.",
            "Exact vendor model snapshot may be undisclosed. Host Docker-client CPU/RSS are not attributed to AX.",
            "Native token/cost limits need complete Provider reporting; global admission limits cannot cap in-flight billing.",
            "A stopped failed container is retained by default, after credential-directory removal; private artifacts must not be uploaded wholesale."])
    try:
        for profile in config["profiles"]:
            for task in manifest["tasks"]:
                for repeat in range(1, config.get("rounds", 3)+1):
                    row = dict(task=task["id"], profile=profile["id"], category=task["format"], round=repeat,
                        status="not_run", reason="offline_plan", agent_executed=False)
                    reason = budget_reason(config, result["runs"], time.monotonic()-started)
                    result["runs"].append(row)
                    if args.execute and not reason:
                        row.update(status="running", reason=None)
                        report(result, output)
                        folder = output/f"{profile['id']}--{task['id']}--{repeat}"
                        row["artifacts"] = str(folder)
                        remaining = config["global_seconds"]-(time.monotonic()-started) if config.get("global_seconds") else float("inf")
                        try:
                            row.update(execute_public(task, profile, config, binary, folder, remaining))
                        except KeyboardInterrupt:
                            row.update(status="interrupted", reason="controller_interrupted", wall_seconds=max(.000001, time.monotonic()-started))
                            raise
                    elif args.execute:
                        row["reason"] = reason
                    report(result, output)
    finally:
        result["metadata"]["wall_seconds"] = time.monotonic()-started
        report(result, output)
    print(output)
    return int(any(r["status"] in {"failed", "ungraded", "interrupted"} for r in result["runs"]))


if __name__ == "__main__":
    raise SystemExit(main())
