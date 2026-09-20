#!/usr/bin/env python3
"""Independent MCP smoke client for the SAAA D5 server (S09).

This script deliberately shares no code with the Rust implementation. It speaks the
Streamable HTTP JSON contract directly and drives initialize -> tools/list ->
tools_search -> tools_describe -> tools_invoke over the loopback socket.

Environment:
  D5_MCP_URL    full endpoint, e.g. http://127.0.0.1:43127/mcp
  D5_MCP_TOKEN  bearer token value (not a file path)

Exit status 0 and the final line D5_SMOKE_OK mean the three entry points answered.
"""

import json
import os
import sys
import urllib.error
import urllib.request

URL = os.environ["D5_MCP_URL"]
TOKEN = os.environ["D5_MCP_TOKEN"]

ACCEPT = "application/json, text/event-stream"


def post(body, session=None):
    data = json.dumps(body).encode("utf-8")
    request = urllib.request.Request(URL, data=data, method="POST")
    request.add_header("Authorization", "Bearer " + TOKEN)
    request.add_header("Accept", ACCEPT)
    request.add_header("Content-Type", "application/json")
    if session:
        request.add_header("Mcp-Session-Id", session)
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            raw = response.read()
            payload = json.loads(raw) if raw else {}
            return response.status, response.headers, payload
    except urllib.error.HTTPError as error:
        raise SystemExit(
            f"HTTP {error.code} for {body.get('method')}: {error.read().decode('utf-8', 'replace')}"
        ) from error


def notification(method, session, params=None):
    body = {"jsonrpc": "2.0", "method": method}
    if params is not None:
        body["params"] = params
    status, _, _ = post(body, session)
    if status != 202:
        raise SystemExit(f"{method} returned {status}, expected 202")


def require(result, pointer):
    current = result
    for key in pointer.split("/"):
        if isinstance(current, list):
            current = current[int(key)]
        else:
            current = current[key]
    return current


def envelope(call_result):
    text = require(call_result, "result/content/0/text")
    return json.loads(text)


def main():
    status, headers, initialize = post(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}},
        }
    )
    assert status == 200, status
    session = headers.get("Mcp-Session-Id")
    assert session, "initialize did not return Mcp-Session-Id"
    assert require(initialize, "result/protocolVersion") == "2025-06-18"

    notification("notifications/initialized", session)

    _, _, listing = post({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}, session)
    names = [tool["name"] for tool in require(listing, "result/tools")]
    assert names == ["tools_search", "tools_describe", "tools_invoke"], names

    _, _, search = post(
        {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "tools_search", "arguments": {"intent": "search notes"}},
        },
        session,
    )
    search_envelope = envelope(search)
    assert search_envelope["ok"], search_envelope
    candidates = search_envelope["data"]["candidates"]
    assert candidates, "search returned no candidates"
    candidate_ref = candidates[0]["candidateRef"]

    _, _, describe = post(
        {
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {"name": "tools_describe", "arguments": {"candidateRef": candidate_ref}},
        },
        session,
    )
    describe_envelope = envelope(describe)
    assert describe_envelope["ok"], describe_envelope
    execution_ref = describe_envelope["data"]["executionRef"]
    assert execution_ref, "describe returned no executionRef"

    _, _, invoke = post(
        {
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "tools_invoke",
                "arguments": {"executionRef": execution_ref, "arguments": {"q": "value"}},
            },
        },
        session,
    )
    invoke_envelope = envelope(invoke)
    assert invoke_envelope["ok"], invoke_envelope
    assert invoke_envelope["data"]["status"] == "succeeded", invoke_envelope

    print("D5_SMOKE_OK")


if __name__ == "__main__":
    try:
        main()
    except AssertionError as error:
        sys.stderr.write(f"smoke assertion failed: {error}\n")
        raise SystemExit(1)
