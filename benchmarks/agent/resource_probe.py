"""Read-only JSON probe for a dedicated worker PID. No model calls or process control.

A pid-file is JSON {"pid":123,"run_id":"..."}. Process identity protects delta
accounting against PID reuse. Linux network counters cover the process's network
namespace, NOT bytes attributable to that PID; use an isolated namespace when
network attribution matters. Windows network counters remain unavailable.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import platform
import socket
import time


def probe(pid):
    result = dict(pid=pid, timestamp=time.time(), hostname=socket.gethostname(), os=platform.platform(),
        cpu_seconds=None, rss_bytes=None, network_bytes=None, process_identity=None, network_scope=None, network_identity=None)
    if platform.system() == "Linux":
        stat = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        result["process_identity"] = f"{pid}:{stat[19]}"
        result["cpu_seconds"] = (int(stat[11])+int(stat[12]))/os.sysconf("SC_CLK_TCK")
        result["rss_bytes"] = int(stat[21])*os.sysconf("SC_PAGE_SIZE")
        network = Path(f"/proc/{pid}/net/dev").read_text().splitlines()[2:]
        result["network_bytes"] = sum(int(line.split(":", 1)[1].split()[0])+int(line.split(":", 1)[1].split()[8]) for line in network)
        result["network_scope"] = "process_network_namespace_including_other_processes_in_namespace"
        machine = Path("/etc/machine-id")
        result["machine_fingerprint"] = hashlib.sha256(machine.read_bytes()).hexdigest() if machine.exists() else None
        boot = Path("/proc/sys/kernel/random/boot_id").read_bytes()
        machine_epoch = hashlib.sha256((machine.read_bytes() if machine.exists() else socket.gethostname().encode())+boot).hexdigest()
        result["network_identity"] = machine_epoch+":"+os.readlink(f"/proc/{pid}/ns/net")
    elif os.name == "nt":
        from ctypes import wintypes as w
        from execution import phase1
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
        kernel.OpenProcess.restype = w.HANDLE
        kernel.CloseHandle.argtypes = [w.HANDLE]
        handle = kernel.OpenProcess(0x1000 | 0x0010, False, pid)
        if not handle:
            raise OSError(ctypes.get_last_error(), "cannot open worker process")
        try:
            proxy = type("ProcessHandle", (), {"_handle":handle})()
            cpu, rss, _ = phase1.resources(proxy)
            result.update(cpu_seconds=cpu, rss_bytes=rss)
            times = [w.FILETIME() for _ in range(4)]
            kernel.GetProcessTimes.argtypes = [w.HANDLE]+[ctypes.POINTER(w.FILETIME)]*4
            if kernel.GetProcessTimes(handle, *[ctypes.byref(t) for t in times]):
                created = (times[0].dwHighDateTime << 32)+times[0].dwLowDateTime
                result["process_identity"] = f"{pid}:{created}"
        finally:
            kernel.CloseHandle(handle)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid-file", type=Path, required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--trace", type=Path)
    args = parser.parse_args()
    identity = json.loads(args.pid_file.read_text(encoding="utf-8-sig"))
    if identity.get("run_id") != args.run_id or not isinstance(identity.get("pid"), int) or identity["pid"] <= 0:
        parser.error("PID file does not belong to requested run")
    result = probe(identity["pid"])
    result["run_id"] = args.run_id
    if args.trace:
        from execution import trace_metrics
        metrics = trace_metrics(args.trace)
        result.update({key:metrics[key] for key in ("input_tokens", "output_tokens", "api_requests", "cost_microusd", "usage_estimated", "cost_estimated")})
        result["usage_identity"] = str(args.trace.resolve())
        result["usage_scope"] = "Goal cumulative snapshots; cross-node imports may overlap"
        if result.get("input_tokens") is not None:
            result["usage_estimated"] = True
        if result.get("cost_microusd") is not None:
            result["cost_estimated"] = True
    print(json.dumps(result))


if __name__ == "__main__":
    main()
