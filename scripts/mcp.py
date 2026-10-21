#!/usr/bin/env python3
"""Tiny MCP client for the running app (stdlib only).

usage: mcp.py <tool> ['{"json": "args"}' | @args.json] [--png out.png]
       mcp.py tools            # list tools
Env: RUSTY_BEAVER_MCP (default http://127.0.0.1:7979/mcp)
"""
import base64, json, os, sys, urllib.error, urllib.request

URL = os.environ.get("RUSTY_BEAVER_MCP", "http://127.0.0.1:7979/mcp")
session = None


def rpc(method, params=None, notify=False):
    global session
    msg = {"jsonrpc": "2.0", "method": method, "params": params or {}}
    if not notify:
        msg["id"] = 1
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if session:
        headers["mcp-session-id"] = session
    req = urllib.request.Request(URL, json.dumps(msg).encode(), headers)
    with urllib.request.urlopen(req, timeout=120) as r:
        session = r.headers.get("mcp-session-id") or session
        body = r.read().decode()
    if notify:
        return None
    for line in body.splitlines():  # SSE or plain JSON
        line = line[5:].strip() if line.startswith("data:") else line.strip()
        if line.startswith("{"):
            m = json.loads(line)
            if "id" in m:
                if "error" in m:  # raised, not exit: smoke.py reports it as that check's failure
                    raise RuntimeError(f"error: {m['error']}")
                return m["result"]
    raise RuntimeError(f"no response: {body[:200]}")


def main():
    args = [a for a in sys.argv[1:]]
    png = None
    if "--png" in args:
        i = args.index("--png")
        png = args[i + 1]
        del args[i : i + 2]
    if not args:
        sys.exit(__doc__)
    rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "mcp.py", "version": "1"}})
    rpc("notifications/initialized", notify=True)
    if args[0] == "tools":
        for t in rpc("tools/list")["tools"]:
            print(f"{t['name']:18} {t['description']}")
        return
    res = rpc("tools/call", {"name": args[0], "arguments": json.loads(open(args[1][1:]).read() if args[1].startswith("@") else args[1]) if len(args) > 1 else {}})
    for c in res.get("content", []):
        if c["type"] == "image":
            out = png or "screenshot.png"
            open(out, "wb").write(base64.b64decode(c["data"]))
            print(f"wrote {out}")
        else:
            print(c.get("text"))
    if res.get("isError"):
        sys.exit(1)


if __name__ == "__main__":
    try:
        main()
    except urllib.error.URLError as e:
        sys.exit(f"Rusty Beaver is not reachable at {URL} ({e.reason}); start it with `make run`")
    except RuntimeError as e:
        sys.exit(str(e))
