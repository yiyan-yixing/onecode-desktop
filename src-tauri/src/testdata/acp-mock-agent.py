#!/usr/bin/env python3
"""按 ACP v1 规范写的 mock agent —— 用来对 ACP 客户端做**协议一致性测试**。

规范来源：https://agentclientprotocol.com/protocol/v1/overview
（initialize / session-setup / prompt-turn / tool-calls 四页的 JSON 形状照抄）

⚠️ 它**不是**真实 ACP Agent。它证明的是"客户端发出的与回应的消息符合 v1 规范"，
**不证明**与某个真实 Agent 互通。本机没有真实 ACP 实现（`claude-agent-acp` /
`codex-acp` 都不存在），这一点在第 11 刀已取证。

用法：
    acp-mock-agent.py <scenario> <observed.json>
场景：
    full        完整一轮（文本片段 + 思考片段 + 工具调用 + 用量 + end_turn）
    permission  发一个 session/request_permission，记录客户端怎么答
    unsupported 发一个 fs/read_text_file 请求（客户端不支持），验证它**答复**而不是沉默
    badversion  initialize 回一个不同的 protocolVersion
    hang        什么都不答，只为验证客户端收尾的墙钟有界
"""
import json
import sys
import time


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def main():
    scenario = sys.argv[1] if len(sys.argv) > 1 else "full"
    observed_path = sys.argv[2] if len(sys.argv) > 2 else "/tmp/acp-observed.json"
    seen = {
        "client_capabilities": None,
        "protocol_version_seen": None,
        "got_session_new_cwd": False,
        "prompt_text": None,
        "permission_outcome": None,
        "permission_option": None,
        "unsupported_got_response": False,
        "unsupported_error_code": None,
    }
    next_request_id = 900

    if scenario == "hang":
        time.sleep(60)
        return

    for raw in sys.stdin:
        raw = raw.strip()
        if not raw:
            continue
        try:
            msg = json.loads(raw)
        except Exception:
            continue
        method = msg.get("method")
        mid = msg.get("id")

        if method == "initialize":
            seen["protocol_version_seen"] = msg["params"]["protocolVersion"]
            seen["client_capabilities"] = msg["params"].get("clientCapabilities")
            version = 99 if scenario == "badversion" else msg["params"]["protocolVersion"]
            send({
                "jsonrpc": "2.0", "id": mid,
                "result": {
                    "protocolVersion": version,
                    "agentCapabilities": {"loadSession": True},
                    "agentInfo": {"name": "mock-agent", "title": "Mock", "version": "0.1.0"},
                    "authMethods": [],
                },
            })
        elif method == "session/new":
            seen["got_session_new_cwd"] = bool(msg["params"].get("cwd"))
            send({"jsonrpc": "2.0", "id": mid, "result": {"sessionId": "sess_mock_1"}})
        elif method == "session/prompt":
            sid = msg["params"]["sessionId"]
            seen["prompt_text"] = msg["params"]["prompt"][0]["text"]

            # 1) 流式文本 + 思考（两条独立的 chunk 流）
            send({"jsonrpc": "2.0", "method": "session/update", "params": {
                "sessionId": sid,
                "update": {"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "正在思考…"}},
            }})
            for piece in ["你好，", "我是 ", "mock agent。"]:
                send({"jsonrpc": "2.0", "method": "session/update", "params": {
                    "sessionId": sid,
                    "update": {"sessionUpdate": "agent_message_chunk",
                               "messageId": "msg_1", "content": {"type": "text", "text": piece}},
                }})

            # 2) 工具调用：先报 title/kind/status，后续 update **只改 status**
            #    （规范：除 toolCallId 外都可选，只带要改的字段）
            send({"jsonrpc": "2.0", "method": "session/update", "params": {
                "sessionId": sid,
                "update": {"sessionUpdate": "tool_call", "toolCallId": "call_1",
                           "name": "read_file", "title": "读取配置", "kind": "read", "status": "pending"},
            }})
            send({"jsonrpc": "2.0", "method": "session/update", "params": {
                "sessionId": sid,
                "update": {"sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "completed"},
            }})

            # 3) 要权限（客户端方法）—— 规范形状照抄
            if scenario == "permission":
                req_id = next_request_id
                next_request_id += 1
                send({"jsonrpc": "2.0", "id": req_id, "method": "session/request_permission", "params": {
                    "sessionId": sid,
                    "toolCall": {"toolCallId": "call_1"},
                    "options": [
                        {"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"},
                        {"optionId": "reject-once", "name": "Reject", "kind": "reject_once"},
                    ],
                }})
                reply = await_response(req_id)
                if reply:
                    outcome = (reply.get("result") or {}).get("outcome") or {}
                    seen["permission_outcome"] = outcome.get("outcome")
                    seen["permission_option"] = outcome.get("optionId")

            # 4) 不支持的客户端方法：验证客户端**答复**而不是沉默
            if scenario == "unsupported":
                req_id = next_request_id
                next_request_id += 1
                seen["_unsupported_id"] = req_id
                send({"jsonrpc": "2.0", "id": req_id, "method": "fs/read_text_file", "params": {
                    "sessionId": sid, "path": "/etc/hosts",
                }})
                reply = await_response(req_id)
                seen["unsupported_got_response"] = reply is not None
                if reply and reply.get("error"):
                    seen["unsupported_error_code"] = reply["error"].get("code")

            # 5) 用量与花费（引擎自报；客户端应照抄）
            send({"jsonrpc": "2.0", "method": "session/update", "params": {
                "sessionId": sid,
                "update": {"sessionUpdate": "usage_update", "used": 53000, "size": 200000,
                           "cost": {"amount": 0.045, "currency": "USD"}},
            }})

            # 6) 先落盘再答复：把"记录"排成最后一步会让它依赖"客户端不立刻杀我"
            write_observed(observed_path, seen)
            send({"jsonrpc": "2.0", "id": mid, "result": {"stopReason": "end_turn"}})
            return


def write_observed(path, seen):
    seen.pop("_unsupported_id", None)
    with open(path, "w") as f:
        json.dump(seen, f)


def await_response(req_id, timeout=10.0):
    """等客户端对某个请求的答复。用最长 timeout 秒，超时返回 None。"""
    deadline = time.time() + timeout
    while time.time() < deadline:
        raw = sys.stdin.readline()
        if not raw:
            return None
        raw = raw.strip()
        if not raw:
            continue
        try:
            msg = json.loads(raw)
        except Exception:
            continue
        if msg.get("id") == req_id and ("result" in msg or "error" in msg):
            return msg
    return None


if __name__ == "__main__":
    main()
