#!/usr/bin/env python3
import json
import os
import sys


def send(value):
    print(json.dumps(value), flush=True)


send({"type": "register",
      "commands": [{"name": "hello", "description": "Say hello"}],
      "tools": [{"name": "ext_echo", "description": "Echo text",
                 "input_schema": {"type": "object"},
                 "capabilities": ["read"]}],
      "events": ["tool_call", "tool_result", "context", "session_start",
                 "session_before_compact", "session_compact", "turn_start", "turn_end",
                 "input", "before_agent_start", "session_shutdown", "session_before_switch",
                 "before_provider_headers", "before_provider_request",
                 "after_provider_response", "agent_settled", "message_end",
                 "session_compact_failed"]})

for line in sys.stdin:
    message = json.loads(line)
    kind = message.get("type")
    if kind == "shutdown":
        break
    if kind == "initialize":
        continue
    reply = {"type": "response", "id": message.get("id", "")}
    if kind == "command":
        reply["message"] = "Hello " + message.get("arguments", "") + \
            " env=" + os.environ.get("NIMINAL_SESSION_ID", "none")
        reply["notification"] = {"level": "info", "message": "command ran"}
        reply["status"] = {"key": "state", "segments": [
            {"text": "ready", "style": "success"}]}
        reply["widget"] = {"key": "work", "content": [
            {"type": "text", "text": "extension widget", "style": "muted"}]}
        reply["entry"] = {"count": 1}
        reply["user_message"] = {"content": "background done", "deliver_as": "follow_up"}
    elif kind == "tool":
        send({"type": "tool_update", "id": message.get("id"), "content": "halfway"})
        reply["content"] = [{"type": "text", "text": "extension tool result"}]
        reply["is_error"] = False
    elif kind == "event" and message.get("event") == "tool_call":
        reply["arguments"] = {"command": "changed"}
    elif kind == "event" and message.get("event") == "tool_result":
        reply["output"] = "rewritten"
        reply["is_error"] = True
    elif kind == "event" and message.get("event") == "context":
        reply["system"] = ["Injected system"]
        reply["messages"] = [{"role": "user", "content": "Injected context"}]
    elif kind == "event" and message.get("event") == "input":
        reply["text"] = message["payload"]["text"] + " transformed"
    elif kind == "event" and message.get("event") == "before_agent_start":
        reply["system_prompt"] = "Task system"
        reply["message"] = {"content": "Persistent extension context"}
    elif kind == "event" and message.get("event") == "session_before_switch":
        reply["allow"] = False
        reply["reason"] = "unsaved work"
    elif kind == "event" and message.get("event") == "before_provider_headers":
        reply["headers"] = {"Authorization": None, "x-test": "yes"}
    elif kind == "event" and message.get("event") == "before_provider_request":
        reply["payload"] = {"model": "replacement"}
    elif kind == "event" and message.get("event") == "message_end":
        reply["text"] = "rewritten answer"
    elif kind == "event" and message.get("event") == "session_before_compact":
        reply["compaction"] = {"summary": "extension summary",
                               "first_kept_index": 1,
                               "details": {"source": "fixture"}}
    send(reply)
