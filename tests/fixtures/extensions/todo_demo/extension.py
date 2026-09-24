#!/usr/bin/env python3
import json
import os
import sys
from pathlib import Path

def state_path():
    session_id = os.environ.get("NIMINAL_SESSION_ID", "default")
    return Path.home() / ".niminal" / "todo_demo" / f"{session_id}.json"

TOOL = {
    "name": "todo",
    "description": "Minimal todo tool for extension tests.",
    "input_schema": {
        "type": "object",
        "properties": {
            "action": {"type": "string",
                         "enum": ["create", "update", "list", "get", "delete", "clear"]},
            "subject": {"type": "string"},
            "description": {"type": "string"},
            "activeForm": {"type": "string"},
            "id": {"type": "integer", "minimum": 1},
            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
        },
        "required": ["action"],
        "additionalProperties": False,
    },
    "capabilities": ["read", "write"],
}


def send(value):
    print(json.dumps(value), flush=True)


def load_state():
    try:
        return json.loads(state_path().read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {"tasks": [], "next_id": 1}


def save_state(state):
    path = state_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")


def open_tasks(state):
    return [task for task in state["tasks"] if task["status"] != "completed"]


def widget(state):
    items = []
    for task in state["tasks"]:
        if task["status"] == "completed":
            state_name = "done"
        elif task["status"] == "in_progress":
            state_name = "active"
        else:
            state_name = "pending"
        items.append({"text": f"#{task['id']} {task['subject']}", "state": state_name})
    actions = []
    for task in open_tasks(state):
        actions.append({"id": f"complete:{task['id']}", "label": f"Complete #{task['id']}"})
    complete = sum(1 for task in state["tasks"] if task["status"] == "completed")
    return {"key": "tasks", "title": f"Todos · {complete}/{len(state['tasks'])} complete",
            "content": [{"type": "list", "items": items},
                        {"type": "text", "text": summary(state), "style": "muted"}],
            "actions": actions}


def summary(state):
    if not state["tasks"]:
        return "No tasks."
    lines = []
    for task in state["tasks"]:
        if task["status"] == "completed":
            continue
        lines.append(f"- #{task['id']} {task['subject']} ({task['status']})")
    return "\n".join(lines) if lines else "No open tasks."


def run_tool(args):
    state = load_state()
    action = args["action"]
    if action == "create":
        task = {"id": state["next_id"], "subject": args["subject"],
                "description": args.get("description", ""), "status": "pending", "activeForm": ""}
        state["next_id"] += 1
        state["tasks"].append(task)
        save_state(state)
        return f"Created [pending] #{task['id']} {task['subject']}", widget(state)
    if action == "update":
        task = next((t for t in state["tasks"] if t["id"] == args["id"]), None)
        if task is None:
            raise ValueError("unknown task")
        task["status"] = args["status"]
        if args.get("activeForm"):
            task["activeForm"] = args["activeForm"]
        save_state(state)
        return f"Updated #{task['id']} to {task['status']}", widget(state)
    if action == "list":
        return summary(state), widget(state)
    if action == "clear":
        count = len(state["tasks"])
        state = {"tasks": [], "next_id": 1}
        save_state(state)
        return f"Cleared {count} tasks.", widget(state)
    raise ValueError(f"unsupported action: {action}")


send({"type": "register",
      "commands": [{"name": "todos", "description": "Show the current task list"}],
      "tools": [TOOL],
      "events": ["session_start"]})

for line in sys.stdin:
    message = json.loads(line)
    kind = message.get("type")
    if kind == "shutdown":
        break
    if kind == "initialize":
        state = load_state()
        if state["tasks"]:
            send({"type": "update", "widget": widget(state)})
        continue
    if kind == "command" and message.get("name") == "todos":
        state = load_state()
        text = "No todos yet." if not state["tasks"] else summary(state)
        send({"type": "response", "id": message["id"], "message": text, "widget": widget(state)})
    elif kind == "tool" and message.get("name") == "todo":
        try:
            text, panel = run_tool(message.get("arguments", {}))
            if message.get("arguments", {}).get("action") == "create":
                pending = [t for t in load_state()["tasks"] if t["status"] != "completed"]
                if pending:
                    text += "\n\nCurrent todo list:\nPending:\n" + "\n".join(
                        f"- #{t['id']} {t['subject']}" for t in pending)
            send({"type": "response", "id": message["id"],
                  "content": [{"type": "text", "text": text}], "widget": panel})
        except Exception as error:
            send({"type": "response", "id": message["id"],
                  "content": [{"type": "text", "text": str(error)}], "is_error": True})
    elif kind == "event" and message.get("event") == "session_start":
        state = load_state()
        reply = {"type": "response", "id": message.get("id", "")}
        if state["tasks"]:
            reply["widget"] = widget(state)
        send(reply)
    elif kind == "ui_action" and message.get("widget") == "tasks":
        action = message.get("action", "")
        if action.startswith("complete:"):
            task_id = int(action.split(":", 1)[1])
            state = load_state()
            task = next((t for t in state["tasks"] if t["id"] == task_id), None)
            if task is not None:
                task["status"] = "completed"
                save_state(state)
            send({"type": "update", "widget": widget(load_state())})
        elif action == "clear":
            state = {"tasks": [], "next_id": 1}
            save_state(state)
            send({"type": "update", "widget": widget(state)})
