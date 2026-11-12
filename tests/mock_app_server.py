#!/usr/bin/env python3
"""Strict local stdio peer. No network/model/filesystem tool execution."""
import json
import sys
import os
import time


def send(value):
    print(json.dumps(value), flush=True)


def receive():
    line = sys.stdin.readline()
    if not line:
        if os.path.exists("record_cleanup"):
            time.sleep(0.1)
            with open("cleanup_complete", "w") as marker:
                marker.write("done")
        raise SystemExit(0)
    value = json.loads(line)
    assert "jsonrpc" not in value
    return value


def response(request, result):
    send({"id": request["id"], "result": result})


request = receive()
assert request["method"] == "initialize"
response(request, {"userAgent": "mock/1"})
assert receive()["method"] == "initialized"
request = receive()
assert request["method"] in ("thread/start", "thread/resume")
assert request["params"]["approvalPolicy"] == "on-request"
assert request["params"]["approvalsReviewer"] == "user"
response(request, {"thread": {"id": "t1", "turns": []}})
turn = 0
while True:
    request = receive()
    assert request["method"] == "turn/start", request
    assert request["params"]["threadId"] == "t1"
    turn += 1
    tid = f"turn-{turn}"
    prompt = request["params"]["input"][0]["text"]
    if prompt == "crash":
        sys.exit(17)
    if prompt == "malformed":
        print("not json", flush=True)
        continue
    send({"method": "turn/started", "params": {"threadId": "t1", "turn": {"id": tid, "status": "inProgress"}}})
    response(request, {"turn": {"id": tid, "status": "inProgress"}})
    if prompt == "duplicate-approval":
        for command in ("echo safe", "echo changed"):
            send({"id": "duplicate", "method": "item/commandExecution/requestApproval", "params": {"threadId": "t1", "turnId": tid, "itemId": "cmd", "command": command}})
        receive()  # client must disconnect without approving either scope
        raise AssertionError("duplicate approval ID accepted")
    if prompt == "child-approval":
        send({"id": "child-request", "method": "item/fileChange/requestApproval", "params": {"threadId": "child", "turnId": "child-turn", "itemId": "edit"}})
        send({"method": "serverRequest/resolved", "params": {"threadId": "child", "requestId": "child-request"}})
        send({"method": "item/completed", "params": {"threadId": "t1", "item": {"id": "marker", "type": "agentMessage", "text": "child resolved"}}})
    if prompt in ("interrupt", "child-approval"):
        request = receive()
        assert request["method"] == "turn/interrupt"
        assert request["params"] == {"threadId": "t1", "turnId": tid}
        response(request, {})
        send({"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": tid, "status": "interrupted", "items": []}}})
        continue
    if prompt == "unsupported":
        send({"id": "question", "method": "item/tool/requestUserInput", "params": {"questions": []}})
        request = receive()
        assert request["id"] == "question"
        assert request["error"]["code"] == -32601
    else:
        send({"method": "item/started", "params": {"threadId": "t1", "turnId": tid, "item": {"id": f"cmd-{turn}", "type": "commandExecution", "command": "echo hello", "status": "inProgress"}}})
        # Numeric IDs overlap client request IDs deliberately; separate directions.
        approval_id = 3 if turn % 2 else "3"
        send({"id": approval_id, "method": "item/commandExecution/requestApproval", "params": {"threadId": "t1", "turnId": tid, "itemId": f"cmd-{turn}", "command": "echo hello", "cwd": "/mock", "availableDecisions": ["accept", "decline"]}})
        request = receive()
        assert request["id"] == approval_id and type(request["id"]) is type(approval_id)
        assert request["result"]["decision"] in ("accept", "decline")
        send({"method": "item/commandExecution/outputDelta", "params": {"threadId": "t1", "itemId": f"cmd-{turn}", "delta": "hello\n"}})
        send({"method": "item/agentMessage/delta", "params": {"threadId": "t1", "itemId": f"a-{turn}", "delta": "你好"}})
        send({"method": "item/completed", "params": {"threadId": "t1", "item": {"id": f"a-{turn}", "type": "agentMessage", "text": "你好，完成。"}}})
        send({"method": "turn/diff/updated", "params": {"threadId": "t1", "turnId": tid, "diff": "--- a/test\n+++ b/test\n+hello\n"}})
    send({"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": tid, "status": "completed", "items": []}}})
