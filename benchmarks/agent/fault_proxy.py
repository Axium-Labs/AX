"""Opt-in HTTP fault gateway for real provider traffic; never fabricates success.

Run separately, point a dedicated compatible AX provider at the printed local URL.
TLS upstream is verified. Request/authorization bodies are never written to logs.
This is controlled transport fault injection, NOT an alternate model provider.
"""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import threading
import time
import socket
from contextlib import contextmanager
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import Request, urlopen


class FaultGateway(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address, upstream, fault, count, delay, log):
        super().__init__(address, Handler)
        self.upstream = upstream.rstrip("/")
        self.fault, self.remaining, self.delay, self.log = fault, count, delay, Path(log)
        self.lock = threading.Lock()
        self.stopping = threading.Event()
        self.upstream_timeout = 180
        self.allow_http_upstream = False


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def log_message(self, *_):
        pass

    def do_POST(self):
        server = self.server
        with server.lock:
            injected = server.remaining > 0
            server.remaining -= int(injected)
        started = time.monotonic()
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        status = None
        response_bytes = 0
        received_at = time.time()
        try:
            if injected:
                if server.fault == "timeout":
                    if server.stopping.wait(server.delay):
                        return
                if server.fault == "disconnect":
                    status = "injected_connection_reset"
                    self.connection.shutdown(socket.SHUT_RDWR)
                    self.connection.close()
                    return
                status = 429 if server.fault == "429" else 503
                self.send_response(status)
                self.send_header("Retry-After", "1")
                self.end_headers()
                self.wfile.write(b'{"error":{"message":"AX-Bench injected transient transport fault"}}')
            else:
                headers = {k:v for k,v in self.headers.items() if k.lower() not in {"host", "connection", "content-length"}}
                request = Request(server.upstream+self.path, data=body, headers=headers, method="POST")
                try:
                    response = urlopen(request, timeout=server.upstream_timeout)
                except HTTPError as exc:
                    response = exc
                with response:
                    status = response.status
                    self.send_response(status)
                    for k,v in response.headers.items():
                        if k.lower() not in {"transfer-encoding", "connection", "content-length", "server", "date"}:
                            self.send_header(k, v)
                    self.end_headers()
                    while chunk := response.read1(8192):
                        self.wfile.write(chunk)
                        self.wfile.flush()
                        response_bytes += len(chunk)
        except (OSError, TimeoutError) as exc:
            status = f"transport_error:{type(exc).__name__}"
        finally:
            with server.lock, server.log.open("a", encoding="utf-8") as stream:
                stream.write(json.dumps(dict(timestamp=time.time(), started_at=received_at, injected=injected, fault=server.fault if injected else None,
                    status=status, request_bytes=len(body), response_bytes=response_bytes, wall_seconds=time.monotonic()-started))+"\n")


ENDPOINT_BINDINGS = {
    "openai": ("OPENAI_API_URL", "/v1/responses"),
    "anthropic": ("ANTHROPIC_BASE_URL", ""),
    "google": ("GOOGLE_API_BASE_URL", "/v1beta"),
    "radius": ("RADIUS_BASE_URL", ""),
    "azure-openai-responses": ("AZURE_OPENAI_BASE_URL", "/openai/v1/responses"),
}


def fault_prerequisite(profile):
    settings = profile.get("fault_gateway", {})
    if profile.get("provider") not in ENDPOINT_BINDINGS:
        return "selected Provider has no production endpoint override; select a supported endpoint or configure external network fault isolation"
    parsed = urlsplit(settings.get("upstream", ""))
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password or parsed.path not in {"", "/"} or parsed.query or parsed.fragment:
        return "profile.fault_gateway.upstream must be an HTTPS origin without embedded credentials"
    return None


@contextmanager
def provider_fault(profile, task, folder, env, timeout):
    """Route only the selected production Provider through a bounded fault gateway."""
    reason = fault_prerequisite(profile)
    if reason:
        raise ValueError(reason)
    settings = profile["fault_gateway"]
    plan = task["fault"]
    key, default_path = ENDPOINT_BINDINGS[profile["provider"]]
    path = settings.get("endpoint_path", default_path)
    if path and (not path.startswith("/") or "?" in path or "#" in path):
        raise ValueError("fault gateway endpoint_path must be an absolute URL path")
    with FaultGateway(("127.0.0.1", 0), settings["upstream"], plan["kind"], plan.get("count", 1), plan.get("delay_seconds", 20), folder/"faults.jsonl") as server:
        server.upstream_timeout = max(.1, min(180, timeout))
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        original = env.get(key)
        env[key] = f"http://127.0.0.1:{server.server_port}{path}"
        try:
            yield server
        finally:
            server.stopping.set()
            server.shutdown()
            worker.join(timeout=5)
            if original is None:
                env.pop(key, None)
            else:
                env[key] = original


def fault_observations(path):
    entries = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()] if path.exists() else []
    injected = [x for x in entries if x["injected"]]
    boundary = min((x.get("started_at", x["timestamp"]) for x in injected), default=None)
    recovered = [x for x in entries if not x["injected"] and isinstance(x.get("status"), int) and 200 <= x["status"] < 300 and (boundary is None or x.get("started_at", x["timestamp"]) >= boundary)]
    return dict(fault_injected=bool(injected), injected_count=len(injected),
        recovery_transport_observed=bool(injected and recovered),
        recovery_seconds=min(x["timestamp"] for x in recovered)-boundary if injected and recovered else None,
        network_bytes=sum(x.get("request_bytes", 0)+x.get("response_bytes", 0) for x in entries),
        network_scope="gateway HTTP payload bytes only; excludes TLS/TCP headers, tools and other network traffic",
        fault_entries=entries)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream", required=True)
    parser.add_argument("--fault", choices=["429", "503", "timeout", "disconnect"], required=True)
    parser.add_argument("--count", type=int, default=1)
    parser.add_argument("--delay", type=float, default=120)
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--log", type=Path, required=True)
    args = parser.parse_args()
    if urlsplit(args.upstream).scheme != "https" or args.count < 1 or args.delay <= 0:
        parser.error("requires HTTPS upstream, positive count and delay")
    with FaultGateway(("127.0.0.1", args.port), args.upstream, args.fault, args.count, args.delay, args.log) as server:
        print(f"http://127.0.0.1:{server.server_port}", flush=True)
        server.serve_forever()


if __name__ == "__main__":
    main()
