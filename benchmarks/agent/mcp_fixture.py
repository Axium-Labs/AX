"""Real stdio MCP fixture service; a controlled external tool, not a mock AX."""
import json
from pathlib import Path
import sys
import time

RECORDS = {"cobalt": {"value": 7319, "next": "raven"}, "raven": {"value": 42, "next": "amber"}, "amber": {"value": 17, "next": None}}


def main():
    log = Path(sys.argv[1])
    failures = int(sys.argv[2]) if len(sys.argv) > 2 else 0
    calls = 0
    for line in sys.stdin:
        request = json.loads(line)
        method = request.get("method")
        if "id" not in request:
            continue
        error = None
        if method == "initialize":
            result = {"protocolVersion": request.get("params", {}).get("protocolVersion", "2024-11-05"), "capabilities": {"tools": {}}, "serverInfo": {"name": "ax-bench-fixture", "version": "1"}}
        elif method == "ping":
            result = {}
        elif method == "tools/list":
            result = {"tools": [{"name": "lookup", "description": "Look up the exact record for a key; returns value and next key.", "annotations": {"readOnlyHint": True}, "inputSchema": {"type": "object", "properties": {"key": {"type": "string"}}, "required": ["key"], "additionalProperties": False}}]}
        elif method == "tools/call":
            calls += 1
            key = request.get("params", {}).get("arguments", {}).get("key")
            error = "injected transient tool failure" if calls <= failures else "unknown key" if key not in RECORDS else None
            result = {"content": [{"type": "text", "text": error or json.dumps(RECORDS[key])}], "isError": error is not None}
        else:
            result = None
        with log.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(dict(timestamp=time.time(), method=method, params=request.get("params"), error=error))+"\n")
        response = {"jsonrpc": "2.0", "id": request["id"]}
        response.update(result=result) if result is not None else response.update(error={"code": -32601, "message": "unknown method"})
        print(json.dumps(response), flush=True)


if __name__ == "__main__":
    main()
