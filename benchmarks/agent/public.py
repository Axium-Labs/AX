"""Pinned public dataset manifests and official SWE prediction export.

Importers never copy reference solutions/test patches into agent specifications.
Use public_execution.py to execute manifests with production AX and official tests.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

SOURCES = {
    "swe-bench-verified": "https://www.swebench.com/SWE-bench/guides/evaluation/",
    "terminal-bench": "https://docs.harborframework.com/tasks/overview",
}


def fingerprint_tree(directory):
    root = directory.resolve()
    result = {}
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"dataset symlinks are not accepted: {path}")
        if path.is_file() and ".git" not in path.relative_to(root).parts:
            result[path.relative_to(root).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return result


def safe_id(value):
    if not isinstance(value, str) or not value or any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_.-" for c in value) or value in {".", ".."}:
        raise ValueError("unsafe public task id")
    return value


def import_swe(source, revision):
    if not revision:
        raise ValueError("pinned dataset revision required")
    rows = [json.loads(s) for s in source.read_text(encoding="utf-8").splitlines() if s.strip()]
    tasks = [dict(id=safe_id(r["instance_id"]), repository=r["repo"], base_commit=r["base_commit"],
        prompt=r["problem_statement"], format="swe-bench", status="ready_for_prepared_container") for r in rows]
    if len({t["id"] for t in tasks}) != len(tasks):
        raise ValueError("duplicate instance ids")
    return dict(source="swe-bench-verified", dataset_revision=revision,
        dataset_sha256=hashlib.sha256(source.read_bytes()).hexdigest(), official_validator=SOURCES["swe-bench-verified"], tasks=tasks)


def import_terminal(directory, revision):
    if not revision:
        raise ValueError("pinned dataset commit required")
    tasks = []
    for root in sorted(p for p in directory.iterdir() if p.is_dir()):
        if root.is_symlink():
            raise ValueError("symlinked task directories are not accepted")
        legacy, modern = root/"task.yaml", root/"task.toml"
        if not legacy.exists() and not modern.exists():
            continue
        task = dict(id=safe_id(root.name), task_directory=str(root.resolve()), source_files=fingerprint_tree(root))
        if modern.exists():
            required = ["instruction.md", "environment/Dockerfile", "tests/test.sh"]
            task.update(format="terminal-v2", prompt=(root/"instruction.md").read_text(encoding="utf-8") if (root/"instruction.md").exists() else None,
                        tests_source="tests", verifier_script="/tests/test.sh", reward_path="/logs/verifier/reward.txt")
        else:
            # Data parsing only; official legacy grading still uses its pinned parser.
            try:
                import yaml
            except ImportError as exc:
                raise ValueError("legacy Terminal import requires PyYAML, also used by upstream") from exc
            spec = yaml.safe_load(legacy.read_text(encoding="utf-8"))
            required = ["run-tests.sh", "tests"]
            task.update(format="terminal-v1", prompt=spec["instruction"], parser_name=spec.get("parser_name", "pytest"),
                        tests_source="tests", verifier_script="/axbench/run-tests.sh")
        task["missing"] = [f for f in required if not (root/f).exists()]
        task["status"] = "ready_for_prepared_container" if not task["missing"] else "unsupported_layout"
        tasks.append(task)
    if not tasks:
        raise ValueError("no Terminal task.yaml/task.toml tasks found")
    return dict(source="terminal-bench", dataset_revision=revision, official_validator=SOURCES["terminal-bench"], tasks=tasks)


def export_predictions(manifest, attempts, model_name):
    known = {t["id"]: t for t in manifest["tasks"]}
    predictions = []
    for instance, workspace in attempts.items():
        task = known[instance]
        workspace = Path(workspace).resolve()
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=workspace, text=True).strip()
        if head != task["base_commit"]:
            raise ValueError(f"{instance}: workspace HEAD differs from pinned base_commit")
        patch = subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=workspace, text=True)
        untracked = subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard"], cwd=workspace, text=True).strip()
        if untracked:
            raise ValueError(f"{instance}: untracked files require explicit review before prediction export")
        predictions.append(dict(instance_id=instance, model_name_or_path=model_name, model_patch=patch))
    return predictions


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=["import-swe", "import-terminal", "export-swe"])
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--revision")
    parser.add_argument("--attempts", type=Path)
    parser.add_argument("--model-name")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output must not exist")
    if args.operation == "export-swe":
        if not args.attempts or not args.model_name:
            parser.error("export requires attempts mapping and model name")
        rows = export_predictions(json.loads(args.source.read_text(encoding="utf-8-sig")), json.loads(args.attempts.read_text(encoding="utf-8-sig")), args.model_name)
        args.output.write_text("\n".join(json.dumps(r) for r in rows)+"\n", encoding="utf-8")
    else:
        result = import_swe(args.source, args.revision) if args.operation == "import-swe" else import_terminal(args.source, args.revision)
        args.output.write_text(json.dumps(result, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
