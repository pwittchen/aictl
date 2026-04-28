#!/usr/bin/env python3
"""Minimal MCP server exposing a single `add` tool.

Smoke-test for the MCP client wiring. Speaks line-delimited JSON-RPC 2.0
over stdin/stdout — the same dialect every reference MCP server uses —
and only implements the four methods aictl drives in Phase 1:
`initialize`, `tools/list`, `tools/call`, `shutdown`.

Wire it up by adding the matching entry to `~/.aictl/mcp.json`:

    {
      "mcpServers": {
        "tiny_add": {
          "command": "python3",
          "args": ["/absolute/path/to/server.py"]
        }
      }
    }

Then `aictl --list-mcp` should show `tiny_add  ready  1 tools` with the
`mcp__tiny_add__add` tool. Calling it from the agent looks like:

    <tool name="mcp__tiny_add__add">
    {"a": 2, "b": 3}
    </tool>
"""
import json
import sys


def reply(req_id, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req_id, "result": result}) + "\n")
    sys.stdout.flush()


TOOL = {
    "name": "add",
    "description": "Add two numbers and return the sum.",
    "inputSchema": {
        "type": "object",
        "properties": {
            "a": {"type": "number", "description": "First addend"},
            "b": {"type": "number", "description": "Second addend"},
        },
        "required": ["a", "b"],
    },
}


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = msg.get("method")
        req_id = msg.get("id")
        if method == "initialize":
            reply(req_id, {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "tiny_add", "version": "0.1.0"},
            })
        elif method == "notifications/initialized":
            # Notification, no response.
            continue
        elif method == "tools/list":
            reply(req_id, {"tools": [TOOL]})
        elif method == "tools/call":
            args = msg.get("params", {}).get("arguments", {})
            try:
                total = float(args["a"]) + float(args["b"])
            except (KeyError, TypeError, ValueError) as e:
                reply(req_id, {
                    "content": [{"type": "text", "text": f"bad arguments: {e}"}],
                    "isError": True,
                })
                continue
            # Render integers without a trailing `.0` for cleaner output.
            text = str(int(total)) if total.is_integer() else str(total)
            reply(req_id, {"content": [{"type": "text", "text": text}]})
        elif method == "shutdown":
            reply(req_id, {})
            break


if __name__ == "__main__":
    main()
