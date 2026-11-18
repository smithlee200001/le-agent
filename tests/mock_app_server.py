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


def stale_frames(tid):
    for method in ("item/started", "item/completed", "item/agentMessage/delta",
                   "item/commandExecution/outputDelta", "turn/diff/updated",
                   "turn/completed", "turn/started"):
        send({"method": method, "params": {"threadId": "t1", "turnId": tid,
              "itemId": "history", "delta": "stale", "diff": "stale diff",
              "item": {"id": "history", "type": "agentMessage", "text": "stale"},
              "turn": {"id": tid, "status": "completed"}}})


def gate(name, tid):
    send({"id": name, "method": "item/fileChange/requestApproval",
          "params": {"threadId": "t1", "turnId": tid, "itemId": name,
                     "availableDecisions": ["decline"]}})
    request = receive()
    assert request == {"id": name, "result": {"decision": "decline"}}, request


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
    if prompt.startswith("start-gated"):
        gate("start-gate", tid)
        if prompt == "start-gated-completed":
            send({"method": "turn/completed", "params": {"threadId": "t1",
                  "turn": {"id": tid, "status": "completed"}}})
            response(request, {"turn": {"id": tid, "status": "completed"}})
            gate("completed-gate", tid)
            continue
        if prompt == "start-gated-failure":
            send({"id": request["id"], "error": {"code": -1, "message": "start failed"}})
            continue
        # Only the RPC response supplies the identity: cancellation must still work.
        response(request, {"turn": {"id": tid, "status": "inProgress"}})
        interrupt = receive()
        assert interrupt["method"] == "turn/interrupt"
        assert interrupt["params"] == {"threadId": "t1", "turnId": tid}
        response(interrupt, {})
        send({"method": "turn/completed", "params": {"threadId": "t1",
              "turn": {"id": tid, "status": "interrupted"}}})
        continue
    if prompt == "stale-b":
        stale_frames(f"turn-{turn - 1}")
    send(
        {
            "method": "turn/started",
            "params": {"threadId": "t1", "turn": {"id": tid, "status": "inProgress"}},
        }
    )
    response(request, {"turn": {"id": tid, "status": "inProgress"}})
    if prompt == "history-a":
        send({"method": "item/completed", "params": {"threadId": "t1", "turnId": tid,
              "item": {"id": "history", "type": "agentMessage", "text": "confirmed"}}})
        send({"method": "turn/completed", "params": {"threadId": "t1",
              "turn": {"id": tid, "status": "completed"}}})
        continue
    if prompt == "stale-b":
        stale_frames(f"turn-{turn - 1}")
        send({"method": "item/completed", "params": {"threadId": "t1", "turnId": tid,
              "item": {"id": "marker", "type": "agentMessage", "text": "B ready"}}})
    if prompt.startswith("interrupt-gated"):
        request = receive()
        assert request["method"] == "turn/interrupt"
        assert request["params"] == {"threadId": "t1", "turnId": tid}
        response(request, {})
        # Explicit wire gate: the test sends another Interrupt after observing ACK,
        # then declines this gate. A duplicate interrupt must not precede the reply.
        gate("terminal-gate", tid)
        if prompt == "interrupt-gated-disconnect":
            sys.exit(17)
        status = {"interrupt-gated-completed": "completed", "interrupt-gated-failed": "failed"}.get(prompt, "interrupted")
        send({"method": "turn/completed", "params": {"threadId": "t1",
              "turn": {"id": tid, "status": status, "items": []}}})
        continue
    if prompt == "duplicate-approval":
        for command in ("echo safe", "echo changed"):
            send(
                {
                    "id": "duplicate",
                    "method": "item/commandExecution/requestApproval",
                    "params": {
                        "threadId": "t1",
                        "turnId": tid,
                        "itemId": "cmd",
                        "command": command,
                    },
                }
            )
        receive()  # client must disconnect without approving either scope
        raise AssertionError("duplicate approval ID accepted")
    if prompt == "child-approval":
        send(
            {
                "id": "child-request",
                "method": "item/fileChange/requestApproval",
                "params": {
                    "threadId": "child",
                    "turnId": "child-turn",
                    "itemId": "edit",
                },
            }
        )
        send(
            {
                "method": "serverRequest/resolved",
                "params": {"threadId": "child", "requestId": "child-request"},
            }
        )
        send(
            {
                "method": "item/completed",
                "params": {
                    "threadId": "t1",
                    "turnId": tid,
                    "item": {
                        "id": "marker",
                        "type": "agentMessage",
                        "text": "child resolved",
                    },
                },
            }
        )
    if prompt in ("interrupt", "child-approval", "stale-b"):
        request = receive()
        assert request["method"] == "turn/interrupt"
        assert request["params"] == {"threadId": "t1", "turnId": tid}
        response(request, {})
        send(
            {
                "method": "turn/completed",
                "params": {
                    "threadId": "t1",
                    "turn": {"id": tid, "status": "interrupted", "items": []},
                },
            }
        )
        continue
    if prompt == "unsupported":
        send(
            {
                "id": "question",
                "method": "item/tool/requestUserInput",
                "params": {"questions": []},
            }
        )
        request = receive()
        assert request["id"] == "question"
        assert request["error"]["code"] == -32601
    else:
        send(
            {
                "method": "item/started",
                "params": {
                    "threadId": "t1",
                    "turnId": tid,
                    "item": {
                        "id": f"cmd-{turn}",
                        "type": "commandExecution",
                        "command": "echo hello",
                        "status": "inProgress",
                    },
                },
            }
        )
        # Numeric IDs overlap client request IDs deliberately; separate directions.
        approval_id = 3 if turn % 2 else "3"
        send(
            {
                "id": approval_id,
                "method": "item/commandExecution/requestApproval",
                "params": {
                    "threadId": "t1",
                    "turnId": tid,
                    "itemId": f"cmd-{turn}",
                    "command": "echo hello",
                    "cwd": "/mock",
                    "availableDecisions": ["accept", "decline"],
                },
            }
        )
        request = receive()
        assert request["id"] == approval_id and type(request["id"]) is type(approval_id)
        assert request["result"]["decision"] in ("accept", "decline")
        send(
            {
                "method": "item/commandExecution/outputDelta",
                "params": {
                    "threadId": "t1",
                    "turnId": tid,
                    "itemId": f"cmd-{turn}",
                    "delta": "hello\n",
                },
            }
        )
        send(
            {
                "method": "item/agentMessage/delta",
                "params": {"threadId": "t1", "turnId": tid, "itemId": f"a-{turn}", "delta": "你好"},
            }
        )
        send(
            {
                "method": "item/completed",
                "params": {
                    "threadId": "t1",
                    "turnId": tid,
                    "item": {
                        "id": f"a-{turn}",
                        "type": "agentMessage",
                        "text": "你好，完成。",
                    },
                },
            }
        )
        send(
            {
                "method": "turn/diff/updated",
                "params": {
                    "threadId": "t1",
                    "turnId": tid,
                    "diff": "--- a/test\n+++ b/test\n+hello\n",
                },
            }
        )
    send(
        {
            "method": "turn/completed",
            "params": {
                "threadId": "t1",
                "turn": {"id": tid, "status": "completed", "items": []},
            },
        }
    )
