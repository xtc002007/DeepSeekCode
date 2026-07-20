#!/usr/bin/env python3
"""Minimal MCP stdio server for integration tests (tools/list + tools/call echo)."""

import json
import sys


def respond(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        req = json.loads(line)
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
                        "serverInfo": {"name": "echo", "version": "0.1.0"},
                    },
                }
            )
        elif method == "tools/list":
            respond(
                {
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "result": {
                        "tools": [
                            {
                                "name": "echo",
                                "description": "Echo a message",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "message": {"type": "string"},
                                    },
                                },
                            }
                        ]
                    },
                }
            )
        elif method == "tools/call":
            params = req.get("params") or {}
            message = (params.get("arguments") or {}).get("message", "")
            respond(
                {
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "result": {
                        "content": [{"type": "text", "text": message}],
                    },
                }
            )


if __name__ == "__main__":
    main()
