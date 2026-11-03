"""Compare two raw.json reports, without enforcing a performance threshold."""
import argparse
import csv
import json
from pathlib import Path
from run import summary


def compare(before, after):
    def grouped(data):
        groups = {}
        for run in data["runs"]:
            key = (run["case"], run["size"], run["concurrency"], run["boundary"])
            groups.setdefault(key, []).extend(run["samples"])
        return {key: summary(samples) for key, samples in groups.items()}
    old, new = grouped(before), grouped(after)
    warnings = []
    for key in ("schema_version", "fixture_version", "os", "cpu", "hardware", "build", "rust", "rustflags", "cargo_encoded_rustflags", "tokio_workers"):
        if before["metadata"].get(key) != after["metadata"].get(key):
            warnings.append(f"metadata differs: {key}")
    for key in ("rounds", "samples", "warmup", "sizes"):
        if before["metadata"]["parameters"].get(key) != after["metadata"]["parameters"].get(key):
            warnings.append(f"parameters differ: {key}")
    old_hashes, new_hashes = [{k.replace("\\", "/"): v for k, v in d["metadata"].get("sha256", {}).items()} for d in (before, after)]
    for path in ("benchmarks/runtime/src/main.rs", "benchmarks/runtime/run.py", "benchmarks/runtime/datasets/message.txt"):
        if old_hashes.get(path) != new_hashes.get(path):
            warnings.append(f"harness or fixture differs: {path}")
    for data in (before, after):
        if data["failures"]:
            warnings.append("input contains failed cases; comparison is incomplete")
    rows = []
    for key in sorted(old.keys() & new.keys()):
        a, b = old[key], new[key]
        rows.append(dict(case=key[0], size=key[1], concurrency=key[2], boundary=key[3],
            before_mean_ns=a["mean_ns"], after_mean_ns=b["mean_ns"],
            mean_change_percent=(b["mean_ns"]/a["mean_ns"]-1)*100,
            before_p95_ns=a["p95_ns"], after_p95_ns=b["p95_ns"]))
    if old.keys() != new.keys():
        warnings.append("case sets differ; only matching cases compared")
    return dict(warnings=warnings, comparisons=rows,
        before_commit=before["metadata"]["git_commit"], after_commit=after["metadata"]["git_commit"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = compare(*[json.loads(p.read_text(encoding="utf-8")) for p in (args.before, args.after)])
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output/"comparison.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    if result["comparisons"]:
        with (args.output/"comparison.csv").open("w", newline="", encoding="utf-8") as f:
            writer = csv.DictWriter(f, fieldnames=list(result["comparisons"][0]))
            writer.writeheader()
            writer.writerows(result["comparisons"])
    lines = ["# AX runtime comparison", "", "Descriptive changes only; no significance claim or CI threshold. Negative mean change means lower measured latency.", "", *result["warnings"], ""]
    lines += [f"- {r['case']} size={r['size']} concurrency={r['concurrency']}: {r['mean_change_percent']:+.2f}% mean latency" for r in result["comparisons"]]
    (args.output/"comparison.md").write_text("\n".join(lines)+"\n", encoding="utf-8")


if __name__ == "__main__":
    main()
