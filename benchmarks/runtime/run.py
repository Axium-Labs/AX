"""Offline runner/reporter. Python standard library only; Python 3.10+."""
import argparse
import csv
import ctypes
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile
import threading
import time
from datetime import datetime, timezone

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent


def command(args, **kwargs):
    return subprocess.check_output(args, cwd=ROOT, text=True, encoding="utf-8", **kwargs).strip()


def windows_resources(process):
    from ctypes import wintypes as w
    class Counters(ctypes.Structure):
        _fields_ = [("cb", w.DWORD), ("faults", w.DWORD)] + [(n, ctypes.c_size_t) for n in
            ("peak", "rss", "qpeak", "quota", "nqpeak", "nquota", "pagefile", "peakfile")]
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    kernel.GetProcessTimes.argtypes = [w.HANDLE] + [ctypes.POINTER(w.FILETIME)] * 4
    psapi.GetProcessMemoryInfo.argtypes = [w.HANDLE, ctypes.POINTER(Counters), w.DWORD]
    handle = w.HANDLE(int(process._handle))
    times = [w.FILETIME() for _ in range(4)]
    ok = kernel.GetProcessTimes(handle, *[ctypes.byref(t) for t in times])
    cpu = sum((t.dwHighDateTime << 32) + t.dwLowDateTime for t in times[2:]) / 10_000_000 if ok else None
    counters = Counters()
    counters.cb = ctypes.sizeof(counters)
    mem_ok = psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb)
    return cpu, counters.rss if mem_ok else None, counters.peak if mem_ok else None


def resources(process):
    if os.name == "nt":
        return windows_resources(process)
    if platform.system() == "Linux":
        try:
            stat = Path(f"/proc/{process.pid}/stat").read_text().rsplit(")", 1)[1].split()
            cpu = (int(stat[11]) + int(stat[12])) / os.sysconf("SC_CLK_TCK")
            status = dict(line.split(":", 1) for line in Path(f"/proc/{process.pid}/status").read_text().splitlines())
            return cpu, int(status["VmRSS"].split()[0])*1024, int(status["VmHWM"].split()[0])*1024
        except (OSError, KeyError):
            pass
    return None, None, None


def measured_process(args, cwd, env, timeout):
    start = time.perf_counter_ns()
    # Files avoid pipe-buffer deadlocks for configurable large sample counts.
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        p = subprocess.Popen(args, cwd=cwd, env=env, stdout=stdout, stderr=stderr)
        readings = []
        stop = threading.Event()
        timed_out = []
        def monitor():
            while not stop.is_set():
                try:
                    readings.append(resources(p))
                except OSError:
                    readings.append((None, None, None))
                if (time.perf_counter_ns()-start)/1e9 > timeout:
                    timed_out.append(True)
                    p.kill()
                    return
                stop.wait(.002)
        watcher = threading.Thread(target=monitor, daemon=True)
        watcher.start()
        p.wait()
        elapsed = time.perf_counter_ns()-start
        stop.set()
        watcher.join()
        if timed_out:
            raise TimeoutError(f"worker timeout: {args}")
        readings.append(resources(p))
        stdout.seek(0)
        stderr.seek(0)
        out, err = stdout.read().decode("utf-8"), stderr.read().decode("utf-8", errors="replace")
    if p.returncode:
        raise RuntimeError(f"exit {p.returncode}: {args}\n{out}\n{err}")
    def maximum(index):
        values = [x[index] for x in readings if x[index] is not None]
        return max(values) if values else None
    cpu = maximum(0)
    return out, {"wall_ns": elapsed, "cpu_seconds": cpu,
        "cpu_percent_one_core": cpu/(elapsed/1e9)*100 if cpu is not None else None,
        "rss_sampled_max_bytes": maximum(1), "peak_rss_bytes": maximum(2),
        "resource_scope": "entire_worker_including_setup_warmup_and_teardown",
        "resource_method": "Win32 process counters" if os.name == "nt" else "sampled /proc (may miss short processes)" if platform.system() == "Linux" else "unavailable",
        "monitor_interval_ms": 2}


def summary(samples):
    values = sorted(x["ns_per_op"] for x in samples)
    elapsed = sum(x["elapsed_ns"] for x in samples)
    return {"mean_ns": statistics.mean(values), "median_ns": statistics.median(values),
        "p95_ns": values[math.ceil(len(values)*.95)-1], "min_ns": values[0], "max_ns": values[-1],
        "throughput_ops_s": sum(x["operations"] for x in samples)*1e9/elapsed,
        "sample_count": len(values)}


def report(result, output):
    output.mkdir(parents=True, exist_ok=True)
    (output/"raw.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    rows = []
    for run in result["runs"]:
        for index, sample in enumerate(run["samples"]):
            resource = run["resources"].get("process_samples", [run["resources"]]*len(run["samples"]))[index]
            rows.append({k:run[k] for k in ("case", "size", "concurrency", "round", "boundary")} | sample | resource)
    if rows:
        with (output/"samples.csv").open("w", newline="", encoding="utf-8") as f:
            writer = csv.DictWriter(f, fieldnames=list(rows[0]))
            writer.writeheader()
            writer.writerows(rows)
    lines = ["# AX-Bench Phase 1 — Runtime Microbenchmark", "", "```json", json.dumps(result["metadata"], ensure_ascii=False, indent=2), "```", "",
        "Latency excludes fixture setup and warmup. Resources cover the whole worker (including setup); CPU is % of one logical core and can exceed 100%. RSS is process working set, not Rust heap. Null telemetry means unavailable, never zero.", "",
        "Fresh-process CLI startup does not flush OS file caches. CLI --version/--help only measures that command, not a fully initialized agent. Startup wall latency includes parent launch/wait; a separate thread samples resources every 2 ms. Kernel init excludes process/Tokio initialization.", "",
        "All mock-labelled results include deterministic boundary costs; direct mock baselines are reported separately and are not subtracted. No model/API, external tool, network, Git worktree or durable child-host execution is measured.", "",
        "P95 uses nearest rank over batch-normalized ns/op, not individual operation tail latency. Throughput = total operations / total timed duration. Three or more independent processes per case; each round has its own fixture directory. No statistical claim from a single host or CI threshold.", "",
        "## Results", "", "Case; size; concurrency; mean / median / P95 / min / max (µs/op); ops/s; maximum worker peak RSS (MiB); boundary", ""]
    groups = {}
    for run in result["runs"]:
        key = (run["case"], run["size"], run["concurrency"], run["boundary"])
        groups.setdefault(key, []).append(run)
    summaries = []
    for (case, size, concurrency, boundary), runs in groups.items():
        stats = summary([s for r in runs for s in r["samples"]])
        peaks = [r["resources"]["peak_rss_bytes"] for r in runs if r["resources"]["peak_rss_bytes"] is not None]
        summaries.append(dict(case=case, size=size, concurrency=concurrency, boundary=boundary, **stats))
        metrics = " / ".join(f"{stats[k]/1000:.3f}" for k in ("mean_ns","median_ns","p95_ns","min_ns","max_ns"))
        peak = f"{max(peaks)/2**20:.2f}" if peaks else "unavailable"
        lines.append(f"- `{case}`; {size}; {concurrency}; {metrics}; {stats['throughput_ops_s']:.1f}; {peak}; {boundary}")
    (output/"summary.json").write_text(json.dumps(summaries, indent=2), encoding="utf-8")
    lines += ["", "## Independent rounds", "", "Each line: case / size / concurrency / round; mean µs/op; worker CPU % of one core; sampled max RSS MiB. Whole-worker telemetry cannot be attributed solely to timed operations.", ""]
    for r in result["runs"]:
        cpu = r["resources"]["cpu_percent_one_core"]
        rss = r["resources"]["rss_sampled_max_bytes"]
        lines.append(f"- {r['case']} / {r['size']} / {r['concurrency']} / {r['round']}: {summary(r['samples'])['mean_ns']/1000:.3f}; CPU {cpu}; RSS {rss/2**20 if rss is not None else None}")
    lines += ["", "## Uncovered / limitations", "", *[f"- {s}" for s in LIMITS], "", "## Failures", "", *[f"- {s}" for s in result["failures"]]]
    if not result["failures"]:
        lines.append("All scheduled workers completed and their embedded correctness assertions passed.")
    (output/"report.md").write_text("\n".join(lines)+"\n", encoding="utf-8")


LIMITS = [
    "Session resume measures effective-context lookup, latest 128-message page and public select_context on fixed model messages; excludes private CLI conversion, identity, full resume and UI.",
    "Context select/token estimate/serde use production APIs. Full model request assembly is covered only indirectly by mock kernel turns. No model-based compression, summarization or quality evaluation.",
    "Memory recall measures memory_index + retrieve_index; excludes private CLI injection, embeddings, remote recall and cold legacy index migration. Memory store is a fixed-key upsert; not growing ingestion.",
    "Subagent lifecycle uses actual prepare/spawn/wait/fork/run_child logic with a mock ChildHost and no-op checkpoints; no durable provisioning, Git isolation, network, process child or crash-recovery claim.",
    "Concurrency uses real AgentSupervisor and kernels with a yielding constant provider. It measures scheduling throughput, not useful agent work or model serving throughput. Tool dispatch includes two mock model calls and the real scheduler.",
    "Resources are worker-level, include fixture seeding and warmup; CPU timer resolution limits short runs. Linux /proc is sampled; macOS resource telemetry currently unavailable. No per-operation CPU allocation or leak/stress/soak claim.",
    "Filesystem caches, antivirus, CPU frequency and background load are uncontrolled; keep host/power mode, fixture version, lockfiles, build flags and parameters identical for comparisons.",
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--warmup", type=int, default=5)
    parser.add_argument("--sizes", type=int, nargs="+", default=[16, 128, 1024])
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.rounds < 3 or args.samples < 2 or args.warmup < 1 or min(args.sizes) < 1 or args.timeout <= 0:
        parser.error("requires >=3 rounds, >=2 samples, >=1 warmup, positive sizes/timeout")
    output = args.output or HERE/"results"/datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output = output.resolve()
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(ROOT/"target")
    # Build always, so an old executable can never silently masquerade as current source.
    for cmd in (["cargo", "build", "--release", "--offline", "--locked", "--manifest-path", str(HERE/"Cargo.toml")],
                ["cargo", "build", "--release", "--offline", "--locked", "-p", "cli"]):
        subprocess.run(cmd, cwd=ROOT, env=env, check=True)
    ext = ".exe" if os.name == "nt" else ""
    worker, cli = ROOT/"target"/"release"/("ax-runtime-bench"+ext), ROOT/"target"/"release"/("ax"+ext)
    hashes = {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in
        [HERE/"Cargo.lock", ROOT/"Cargo.lock", HERE/"src/main.rs", HERE/"run.py", HERE/"datasets/message.txt", worker, cli]}
    diff = subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=ROOT)
    metadata = dict(schema_version=1, fixture_version=1, timestamp_utc=datetime.now(timezone.utc).isoformat(),
        git_commit=command(["git", "rev-parse", "HEAD"]), git_status=command(["git", "status", "--porcelain", "--untracked-files=no"]),
        tracked_diff_sha256=hashlib.sha256(diff).hexdigest(), rust=command(["rustc", "-Vv"]),
        os=platform.platform(), cpu=platform.processor(), logical_cpus=os.cpu_count(),
        build="release opt-level=3 lto=thin codegen-units=1 strip=symbols; offline locked; default features",
        rustflags=env.get("RUSTFLAGS", ""), cargo_encoded_rustflags=env.get("CARGO_ENCODED_RUSTFLAGS", ""),
        python=platform.python_version(), tokio_workers=4, parameters=vars(args) | {"output":str(output)}, sha256=hashes)
    if os.name == "nt":
        metadata["hardware"] = command(["powershell", "-NoProfile", "-Command", "@{CPU=(Get-CimInstance Win32_Processor | Select-Object Name,NumberOfCores,NumberOfLogicalProcessors);RAM=(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory} | ConvertTo-Json -Compress"])
    elif platform.system() == "Linux":
        metadata["hardware"] = Path("/proc/cpuinfo").read_text().split("\n\n")[0] + "\n" + Path("/proc/meminfo").read_text().splitlines()[0]
    else:
        metadata["hardware"] = command(["sysctl", "hw.model", "hw.memsize", "hw.ncpu"])
    # Archive the source identity without publishing any user workspace content.
    metadata["production_source_sha256"] = {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted((ROOT/"crates").rglob("*")) if p.is_file() and p.suffix in (".rs", ".toml")}
    result = dict(metadata=metadata, runs=[], failures=[])
    scaled = ["session_load", "session_resume_local", "session_snapshot_load", "context_select", "context_serialize", "context_tokens", "memory_query", "memory_recall", "memory_store", "tool_register", "tool_lookup", "tool_dispatch"]
    cases = [(c, size, 1) for c in scaled for size in args.sizes]
    cases += [(c, 1, 1) for c in ["kernel_init", "session_create", "mock_tool", "mock_model", "kernel_turn", "subagent_lifecycle"]]
    cases += [("concurrency", size, c) for size in [16, 64] for c in [1, 4, 8]]
    cases += [(c, 1, 1) for c in ["startup_version", "startup_help"]]
    for round_id in range(args.rounds):
        # Rotate to reduce systematic ordering bias; retain the actual order in JSON.
        ordered = cases[round_id:] + cases[:round_id]
        for case, size, concurrency in ordered:
            print(f"round {round_id+1}/{args.rounds}: {case} size={size} concurrency={concurrency}", flush=True)
            try:
                with tempfile.TemporaryDirectory(prefix="ax-bench-") as folder:
                    isolated = env.copy()
                    isolated["AX_HOME"] = str(Path(folder)/"home")
                    isolated["AX_BENCH_WORKDIR"] = folder
                    if case.startswith("startup_"):
                        samples, stats = [], []
                        for i in range(args.warmup+args.samples):
                            out, resource = measured_process([str(cli), "--version" if case == "startup_version" else "--help"], folder, isolated, args.timeout)
                            assert "ax" in out.lower()
                            if i >= args.warmup:
                                samples.append(dict(sample=i-args.warmup, elapsed_ns=resource["wall_ns"], operations=1, ns_per_op=resource["wall_ns"]))
                                stats.append(resource)
                        # Preserve every process counter for startup, aggregate only for overview.
                        resource = dict(stats[0])
                        for k in ("wall_ns", "cpu_seconds", "cpu_percent_one_core", "rss_sampled_max_bytes", "peak_rss_bytes"):
                            values = [r[k] for r in stats if r[k] is not None]
                            resource[k] = max(values) if values else None
                        resource["process_samples"] = stats
                        payload = dict(samples=samples)
                    else:
                        out, resource = measured_process([str(worker), case, str(size), str(args.samples), str(args.warmup), str(concurrency)], folder, isolated, args.timeout)
                        payload = json.loads(out)
                    boundary = "mock-boundary baseline" if case.startswith("mock_") else "runtime + deterministic mocks" if case in ["kernel_init", "tool_register", "tool_lookup", "tool_dispatch", "kernel_turn", "subagent_lifecycle", "concurrency"] else "local production path"
                    result["runs"].append(payload | dict(case=case, size=size, concurrency=concurrency, round=round_id+1, boundary=boundary, resources=resource))
            except Exception as error:
                result["failures"].append(f"round={round_id+1} case={case} size={size} concurrency={concurrency}: {error}")
            report(result, output)
    print(f"Report: {output/'report.md'}; failures={len(result['failures'])}")
    return bool(result["failures"])


if __name__ == "__main__":
    raise SystemExit(main())
