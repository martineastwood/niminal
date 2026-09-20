#include "extensions.hpp"
#include "trust.hpp"

#include <atomic>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>

namespace fs = std::filesystem;
using niminal::app::ExtensionRuntime;
using niminal::app::HookEvent;

int main() {
  auto root = fs::temp_directory_path() / "niminal-extensions-test";
  auto home = root / "home";
  auto dir = root / ".niminal" / "extensions" / "fixture";
  auto tool_dir = root / ".niminal" / "tools" / "echo_json";
  auto broken_tool_dir = root / ".niminal" / "tools" / "broken";
  auto collision_tool_dir = root / ".niminal" / "tools" / "collision";
  fs::remove_all(root);
  fs::create_directories(home);
  fs::create_directories(dir);
  fs::create_directories(tool_dir);
  fs::create_directories(broken_tool_dir);
  fs::create_directories(collision_tool_dir);
  setenv("HOME", home.c_str(), 1);
  niminal::app::set_project_resources_trusted(root, true);

  {
    std::ofstream out(dir / "extension.json");
    out << R"({"name":"fixture","command":["./extension.py"]})";
  }
  {
    std::ofstream out(dir / "extension.py");
    out << R"PY(#!/usr/bin/env python3
import json, sys
def send(value):
    print(json.dumps(value), flush=True)
send({"type":"register",
      "commands":[{"name":"hello","description":"Say hello"}],
      "tools":[{"name":"ext_echo","description":"Echo text",
                "input_schema":{"type":"object"},
                "capabilities":["read"]}],
      "events":["tool_call","tool_result","context","session_start",
                "session_before_compact","session_compact","turn_start","turn_end"]})
for line in sys.stdin:
    message = json.loads(line)
    kind = message.get("type")
    if kind == "shutdown":
        break
    if kind == "initialize":
        continue
    reply = {"type":"response", "id":message.get("id", "")}
    if kind == "command":
        reply["message"] = "Hello " + message.get("arguments", "")
        reply["notification"] = {"level":"info", "message":"command ran"}
        reply["status"] = {"key":"state", "text":"ready"}
        reply["widget"] = {"key":"work", "lines":["extension widget"]}
        reply["entry"] = {"count":1}
        reply["user_message"] = {"content":"background done", "deliver_as":"follow_up"}
    elif kind == "tool":
        reply["content"] = [{"type":"text","text":"extension tool result"}]
        reply["is_error"] = False
    elif kind == "event" and message.get("event") == "tool_call":
        reply["arguments"] = {"command":"changed"}
    elif kind == "event" and message.get("event") == "tool_result":
        reply["output"] = "rewritten"
        reply["is_error"] = True
    elif kind == "event" and message.get("event") == "context":
        reply["system"] = ["Injected system"]
        reply["messages"] = [{"role":"user","content":"Injected context"}]
    elif kind == "event" and message.get("event") == "session_before_compact":
        reply["compaction"] = {"summary":"extension summary",
                               "first_kept_index":1,
                               "details":{"source":"fixture"}}
    send(reply)
)PY";
  }
  fs::permissions(dir / "extension.py",
                  fs::perms::owner_read | fs::perms::owner_write |
                      fs::perms::owner_exec);
  {
    std::ofstream out(tool_dir / "tool.json");
    out << R"({"name":"echo_json","description":"Echo JSON input","command":["./run"],"input_schema":{"type":"object"},"capabilities":["read"]})";
  }
  {
    std::ofstream out(tool_dir / "run");
    out << "#!/bin/sh\ninput=$(cat)\nprintf '{\"received\":%s}\\n' \"$input\"\n";
  }
  fs::permissions(tool_dir / "run",
                  fs::perms::owner_read | fs::perms::owner_write |
                      fs::perms::owner_exec);
  {
    std::ofstream out(broken_tool_dir / "tool.json");
    out << "{not json";
  }
  {
    std::ofstream out(collision_tool_dir / "tool.json");
    out << R"({"name":"bash","description":"Collision","command":["./run"],"input_schema":{"type":"object"}})";
  }

  std::atomic<bool> cancel{false};
  auto runtime = ExtensionRuntime::start(root, "session", &cancel);
  if (runtime->commands().size() != 1) {
    std::cerr << "extension registration failed\n";
    return 1;
  }
  auto command = runtime->invoke("hello", "world");
  if (command.value("message", "") != "Hello world") return 1;
  auto tools = runtime->tools();
  niminal::Tool* persistent = nullptr;
  for (auto& tool : tools)
    if (tool.name == "ext_echo") persistent = &tool;
  if (!persistent || !persistent->read_only || !persistent->extension ||
      persistent->run(nlohmann::json::object()) != "extension tool result")
    return 1;
  niminal::Tool* external = nullptr;
  for (auto& tool : tools)
    if (tool.name == "echo_json") external = &tool;
  if (!external || !external->read_only || !external->extension ||
      external->run(nlohmann::json{{"value", 1}}).find("\"value\":1") ==
          std::string::npos)
    return 1;
  bool warned_broken = false;
  bool warned_collision = false;
  for (const auto& warning : runtime->warnings()) {
    warned_broken = warned_broken || warning.find("broken") != std::string::npos;
    warned_collision = warned_collision || warning.find("bash") != std::string::npos;
  }
  if (!warned_broken || !warned_collision) return 1;

  auto pre = runtime->dispatch(
      HookEvent::tool_call,
      nlohmann::json{{"tool", "bash"},
                     {"arguments", {{"command", "original"}}}});
  if (!pre.has_arguments || pre.arguments.value("command", "") != "changed")
    return 1;
  auto post = runtime->dispatch(
      HookEvent::tool_result,
      nlohmann::json{{"tool", "bash"}, {"arguments", nlohmann::json::object()},
                     {"output", "original"}, {"is_error", false}});
  if (!post.has_output || post.output != "rewritten" ||
      !post.has_is_error || !post.is_error)
    return 1;
  auto context = runtime->dispatch(HookEvent::context, nlohmann::json::object());
  if (context.system != std::vector<std::string>{"Injected system"} ||
      context.messages.size() != 1)
    return 1;
  runtime->dispatch(HookEvent::session_start,
                    niminal::app::session_hook_payload("session", root));
  auto notices = runtime->take_notices();
  if (notices.size() != 1 || notices[0].message != "command ran") return 1;
  if (runtime->status_texts() != std::vector<std::string>{"ready"} ||
      runtime->widget_lines() != std::vector<std::string>{"extension widget"})
    return 1;
  auto entries = runtime->take_entries();
  auto user_messages = runtime->take_user_messages();
  if (entries.size() != 1 || entries[0].data.value("count", 0) != 1 ||
      user_messages.size() != 1 ||
      user_messages[0].content != "background done")
    return 1;
  auto compact = runtime->dispatch(HookEvent::session_before_compact,
                                   nlohmann::json::object());
  if (!compact.has_compaction || compact.summary != "extension summary" ||
      compact.first_kept_index != 1 ||
      compact.details.value("source", "") != "fixture")
    return 1;

  niminal::Agent agent;
  agent.conversation_id = "session";
  niminal::app::install_extension_tools(agent, runtime);
  niminal::app::bind_extensions(agent, runtime, root);
  nlohmann::json arguments{{"command", "original"}};
  std::string reason;
  niminal::ToolCall call{"tool", "bash", R"({"command":"original"})"};
  if (!agent.before_tool(call, arguments, reason) ||
      arguments.value("command", "") != "changed")
    return 1;
  std::string output = "original";
  bool is_error = false;
  agent.after_tool(call, arguments, output, is_error);
  if (output != "rewritten" || !is_error) return 1;
  nlohmann::json messages = nlohmann::json::array(
      {{{"role", "system"},
        {"content", nlohmann::json::array(
                        {{{"type", "text"}, {"text", "Base"}}})}},
       {{"role", "user"}, {"content", "Original"}}});
  agent.augment_context(messages);
  if (messages.size() != 3 ||
      messages[0]["content"].size() != 2 ||
      messages[2].value("content", "") != "Injected context")
    return 1;
  agent.turn_start();
  agent.turn_end(false);
  runtime->stop();

  niminal::app::set_project_resources_trusted(root, false);
  auto blocked = ExtensionRuntime::start(root, "session", &cancel);
  if (!blocked->commands().empty() || !blocked->tools().empty()) return 1;
  blocked->stop();
  fs::remove_all(root);
  return 0;
}
