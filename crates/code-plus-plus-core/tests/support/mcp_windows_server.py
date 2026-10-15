#!/usr/bin/env python3
"""Minimal windows-mcp style stdio server for integration tests.

Exposes desktop-automation-like tools over MCP JSON-RPC line protocol:
`screenshot`, `snapshot`, `click`, `type_text`, `powershell` (mocked: echoes
instead of executing, so tests never touch the real desktop).

Reference: my-notebook `mcp.rs` stdio transport + this repo's
`tests/support/mcp_echo_server.py` line protocol.
"""

import json
import sys


TOOLS = [
    {
        "name": "screenshot",
        "description": "Capture desktop screenshot metadata (mocked for tests)",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "snapshot",
        "description": "Inspect desktop UI tree (mocked for tests)",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "click",
        "description": "Click at coordinates",
        "inputSchema": {
            "type": "object",
            "properties": {
                "x": {"type": "integer"},
                "y": {"type": "integer"},
            },
            "required": ["x", "y"],
        },
    },
    {
        "name": "type_text",
        "description": "Type text",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        },
    },
    {
        "name": "powershell",
        "description": "Run a PowerShell command (mocked: echoed, never executed)",
        "inputSchema": {
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        },
    },
]


def respond(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def text_result(req_id, text):
    respond(
        {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {"content": [{"type": "text", "text": text}]},
        }
    )


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            continue
        req_id = req.get("id")
        method = req.get("method")
        if method == "initialize":
            respond(
                {
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "serverInfo": {"name": "windows-mcp", "version": "0.1.0"},
                    },
                }
            )
        elif method == "notifications/initialized":
            continue  # notification: no response
        elif method == "tools/list":
            respond({"jsonrpc": "2.0", "id": req_id, "result": {"tools": TOOLS}})
        elif method == "tools/call":
            params = req.get("params") or {}
            name = params.get("name", "")
            args = params.get("arguments") or {}
            if name == "screenshot":
                text_result(req_id, "display=1920x1080 primary=true mocked-screenshot-ok")
            elif name == "snapshot":
                text_result(req_id, "windows=1 elements=0 mocked-snapshot-ok")
            elif name == "click":
                text_result(req_id, f"clicked x={args.get('x')} y={args.get('y')}")
            elif name == "type_text":
                text_result(req_id, f"typed {len(str(args.get('text', '')))} chars")
            elif name == "powershell":
                # Never execute: echo back so tests stay hermetic.
                text_result(req_id, f"mocked-powershell: {args.get('command', '')}"[:500])
            else:
                respond(
                    {
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "error": {"code": -32601, "message": f"unknown tool: {name}"},
                    }
                )


if __name__ == "__main__":
    main()
