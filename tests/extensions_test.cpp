#include "extensions.hpp"
#include "session.hpp"
#include "trust.hpp"

#include <atomic>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <future>
#include <iostream>
#include <stdexcept>

namespace fs = std::filesystem;
using niminal::app::ExtensionRuntime;
using niminal::app::HookEvent;

int main() {
  auto root = fs::temp_directory_path() / "niminal-extensions-test";
  auto home = root / "home";
  auto dir = root / ".niminal" / "extensions" / "fixture";
  auto host_dir = root / ".niminal" / "extensions" / "host";
  auto parallel_dir = root / ".niminal" / "extensions" / "parallel";
  auto tool_dir = root / ".niminal" / "tools" / "echo_json";
  auto broken_tool_dir = root / ".niminal" / "tools" / "broken";
  auto collision_tool_dir = root / ".niminal" / "tools" / "collision";
  fs::remove_all(root);
  fs::create_directories(home);
  fs::create_directories(dir);
  fs::create_directories(host_dir);
  fs::create_directories(parallel_dir);
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
        send({"type":"tool_update", "id":message.get("id"), "content":"halfway"})
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
                  fs::perms::owner_read | fs::perms::owner_write | fs::perms::owner_exec);
  {
    std::ofstream out(host_dir / "extension.json");
    out << R"({"name":"host","command":["./extension.py"]})";
  }
  {
    std::ofstream out(host_dir / "extension.py");
    out << R"PY(#!/usr/bin/env python3
import json, sys
def send(value):
    print(json.dumps(value), flush=True)
def host(method, **fields):
    send({"type":"host_request", "id":method, "method":method, **fields})
    for line in sys.stdin:
        response = json.loads(line)
        if response.get("type") == "host_response" and response.get("id") == method:
            return response
    return {"cancelled":True}
def ui(method, **fields):
    send({"type":"ui_request", "id":method, "method":method, **fields})
    for line in sys.stdin:
        response = json.loads(line)
        if response.get("type") == "ui_response" and response.get("id") == method:
            return response
    return {"cancelled":True}
send({"type":"register", "commands":[{"name":"host","description":"Host requests"}]})
for line in sys.stdin:
    message = json.loads(line)
    if message.get("type") == "shutdown":
        break
    if message.get("type") != "command":
        continue
    payload = {}
    if message.get("arguments") == "model":
        payload["model"] = host("model.complete", system_prompt="Summarize",
                                 prompt="history", max_tokens=123)
    elif message.get("arguments") == "ui":
        payload["question"] = ui("question", prompt="Pick one",
                                  options=["Red", "Blue"])
        payload["confirm"] = ui("confirm", prompt="Continue?")
        payload["input"] = ui("input", prompt="Branch?")
        payload["password"] = ui("password", prompt="Token?")
    else:
        payload["info"] = host("session.info")
        payload["name"] = host("session.name", name="handoff source")
        payload["usage"] = host("context.usage")
        payload["editor"] = host("ui.editor", title="Edit handoff", text="draft")
    send({"type":"response", "id":message.get("id"), "payload":payload})
)PY";
  }
  fs::permissions(host_dir / "extension.py",
                  fs::perms::owner_read | fs::perms::owner_write | fs::perms::owner_exec);
  {
    std::ofstream out(parallel_dir / "extension.json");
    out << R"({"name":"parallel","command":["./extension.py"]})";
  }
  {
    std::ofstream out(parallel_dir / "extension.py");
    out << R"PY(#!/usr/bin/env python3
import json, sys
def send(value):
    print(json.dumps(value), flush=True)
json.loads(sys.stdin.readline())
send({"type":"register", "commands":[{"name":"parallel","description":"Parallel"}]})
first = json.loads(sys.stdin.readline())
second = json.loads(sys.stdin.readline())
def result(message):
    return "first" if message.get("arguments") == "one" else "second"
send({"type":"response", "id":second.get("id"), "message":result(second)})
send({"type":"response", "id":first.get("id"), "message":result(first)})
for line in sys.stdin:
    if json.loads(line).get("type") == "shutdown":
        break
)PY";
  }
  fs::permissions(parallel_dir / "extension.py",
                  fs::perms::owner_read | fs::perms::owner_write | fs::perms::owner_exec);
  {
    std::ofstream out(tool_dir / "tool.json");
    out << R"({"name":"echo_json","description":"Echo JSON input","command":["./run"],"input_schema":{"type":"object"},"capabilities":["read"]})";
  }
  {
    std::ofstream out(tool_dir / "run");
    out << "#!/bin/sh\ninput=$(cat)\nprintf '{\"received\":%s}\\n' \"$input\"\n";
  }
  fs::permissions(tool_dir / "run",
                  fs::perms::owner_read | fs::perms::owner_write | fs::perms::owner_exec);
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
  if (runtime->commands().size() != 3) {
    std::cerr << "extension registration failed\n";
    return 1;
  }
  auto command = runtime->invoke("hello", "world");
  if (command.value("message", "") != "Hello world") {
    return 1;
  }
  runtime->set_host_request([](const std::string& method, const nlohmann::json& request) {
    if (method != "model.complete" || request.value("max_tokens", 0) != 123) {
      throw std::runtime_error("unexpected model request");
    }
    return nlohmann::json{
        {"text", "generated"}, {"model", "test/model"}, {"finish_reason", "stop"}};
  });
  auto model_host = runtime->invoke("host", "model");
  if (model_host["payload"]["model"]["result"].value("text", "") != "generated" ||
      model_host["payload"]["model"]["result"].value("model", "") != "test/model") {
    return 1;
  }
  auto first = std::async(std::launch::async, [&] { return runtime->invoke("parallel", "one"); });
  auto second = std::async(std::launch::async, [&] { return runtime->invoke("parallel", "two"); });
  auto first_result = first.get();
  auto second_result = second.get();
  if (first_result.value("message", "") != "first" ||
      second_result.value("message", "") != "second") {
    return 1;
  }
  auto tools = runtime->tools();
  std::vector<std::string> updates;
  runtime->set_tool_update(
      [&updates](const std::string&, const std::string& text) { updates.push_back(text); });
  niminal::Tool* persistent = nullptr;
  for (auto& tool : tools) {
    if (tool.name == "ext_echo") {
      persistent = &tool;
    }
  }
  std::string persistent_output;
  if (persistent != nullptr) {
    persistent_output = persistent->run(nlohmann::json::object());
  }
  if ((persistent == nullptr) || !persistent->read_only || !persistent->extension ||
      persistent_output != "extension tool result") {
    return 1;
  }
  if (updates != std::vector<std::string>{"halfway"}) {
    return 1;
  }
  niminal::Tool* external = nullptr;
  for (auto& tool : tools) {
    if (tool.name == "echo_json") {
      external = &tool;
    }
  }
  if ((external == nullptr) || !external->read_only || !external->extension ||
      external->run(nlohmann::json{{"value", 1}}).find("\"value\":1") == std::string::npos) {
    return 1;
  }
  bool warned_broken = false;
  bool warned_collision = false;
  for (const auto& warning : runtime->warnings()) {
    warned_broken = warned_broken || warning.find("broken") != std::string::npos;
    warned_collision = warned_collision || warning.find("bash") != std::string::npos;
  }
  if (!warned_broken || !warned_collision) {
    return 1;
  }

  auto pre =
      runtime->dispatch(HookEvent::tool_call,
                        nlohmann::json{{"tool", "bash"}, {"arguments", {{"command", "original"}}}});
  if (!pre.has_arguments || pre.arguments.value("command", "") != "changed") {
    return 1;
  }
  auto post = runtime->dispatch(HookEvent::tool_result,
                                nlohmann::json{{"tool", "bash"},
                                               {"arguments", nlohmann::json::object()},
                                               {"output", "original"},
                                               {"is_error", false}});
  if (!post.has_output || post.output != "rewritten" || !post.has_is_error || !post.is_error) {
    return 1;
  }
  auto context = runtime->dispatch(HookEvent::context, nlohmann::json::object());
  if (context.system != std::vector<std::string>{"Injected system"} ||
      context.messages.size() != 1) {
    return 1;
  }
  runtime->dispatch(HookEvent::session_start, niminal::app::session_hook_payload("session", root));
  auto notices = runtime->take_notices();
  if (notices.size() != 1 || notices[0].message != "command ran") {
    return 1;
  }
  if (runtime->status_texts() != std::vector<std::string>{"ready"} ||
      runtime->widget_lines() != std::vector<std::string>{"extension widget"}) {
    return 1;
  }
  auto entries = runtime->take_entries();
  auto user_messages = runtime->take_user_messages();
  if (entries.size() != 1 || entries[0].data.value("count", 0) != 1 || user_messages.size() != 1 ||
      user_messages[0].content != "background done") {
    return 1;
  }
  auto compact = runtime->dispatch(HookEvent::session_before_compact, nlohmann::json::object());
  if (!compact.has_compaction || compact.summary != "extension summary" ||
      compact.first_kept_index != 1 || compact.details.value("source", "") != "fixture") {
    return 1;
  }

  niminal::Agent agent;
  agent.conversation_id = "session";
  niminal::app::install_extension_tools(agent, runtime);
  niminal::app::Session session;
  session.id = "session";
  session.workspace = root.string();
  session.persist = false;
  session.add_user("existing context");
  niminal::app::bind_extensions(agent, runtime, root, {}, &session);
  niminal::app::ExtensionUiCallbacks ui;
  ui.editor = [](const std::string& title, const std::string& text) {
    if (title != "Edit handoff" || text != "draft") {
      throw std::runtime_error("unexpected editor request");
    }
    return std::string("edited handoff");
  };
  runtime->set_ui_callbacks(std::move(ui));
  niminal::app::ExtensionUiCallbacks dialogs;
  dialogs.question = [](const std::string& prompt, const std::vector<std::string>& options) {
    if (prompt == "Pick one" && options.size() == 2) {
      return options[1];
    }
    if (prompt == "Continue?" && options.size() == 2) {
      return std::string("Yes");
    }
    throw std::runtime_error("unexpected question request");
  };
  dialogs.input = [](const std::string& prompt, bool secret) {
    if (prompt == "Branch?" && !secret) {
      return std::string("feature");
    }
    if (prompt == "Token?" && secret) {
      return std::string("secret-token");
    }
    throw std::runtime_error("unexpected input request");
  };
  runtime->set_ui_callbacks(std::move(dialogs));
  auto ui_host = runtime->invoke("host", "ui");
  if (ui_host["payload"]["question"]["answer"] != "Blue" ||
      ui_host["payload"]["confirm"]["confirmed"] != true ||
      ui_host["payload"]["input"]["answer"] != "feature" ||
      ui_host["payload"]["password"]["answer"] != "secret-token") {
    return 1;
  }
  niminal::app::ExtensionUiCallbacks editor;
  editor.editor = [](const std::string& title, const std::string& text) {
    if (title != "Edit handoff" || text != "draft") {
      throw std::runtime_error("unexpected editor request");
    }
    return std::string("edited handoff");
  };
  runtime->set_ui_callbacks(std::move(editor));
  auto host = runtime->invoke("host", "session");
  if (host["payload"]["info"]["result"].value("id", "") != "session" ||
      host["payload"]["info"]["result"].value("event_count", 0) != 1 ||
      host["payload"]["name"]["result"].value("name", "") != "handoff source" ||
      host["payload"]["usage"]["result"].value("tokens", 0) <= 0 ||
      host["payload"]["usage"]["result"].value("limit", 0) <= 0 ||
      host["payload"]["editor"]["result"].value("text", "") != "edited handoff") {
    return 1;
  }
  nlohmann::json arguments{{"command", "original"}};
  std::string reason;
  niminal::ToolCall call{"tool", "bash", R"({"command":"original"})"};
  if (!agent.before_tool(call, arguments, reason) || arguments.value("command", "") != "changed") {
    return 1;
  }
  std::string output = "original";
  bool is_error = false;
  agent.after_tool(call, arguments, output, is_error);
  if (output != "rewritten" || !is_error) {
    return 1;
  }
  nlohmann::json messages = nlohmann::json::array(
      {{{"role", "system"},
        {"content", nlohmann::json::array({{{"type", "text"}, {"text", "Base"}}})}},
       {{"role", "user"}, {"content", "Original"}}});
  agent.augment_context(messages);
  if (messages.size() != 3 || messages[0]["content"].size() != 2 ||
      messages[2].value("content", "") != "Injected context") {
    return 1;
  }
  agent.turn_start();
  agent.turn_end(false);
  runtime->stop();

  niminal::app::set_project_resources_trusted(root, false);
  auto blocked = ExtensionRuntime::start(root, "session", &cancel);
  if (!blocked->commands().empty() || !blocked->tools().empty()) {
    return 1;
  }
  blocked->stop();

  // The external editor must actually run and its buffer must round-trip.
  auto script = root / "editor.sh";
  {
    std::ofstream out(script);
    out << "#!/bin/sh\nprintf 'edited draft' > \"$1\"\n";
  }
  fs::permissions(script, fs::perms::owner_exec | fs::perms::owner_read | fs::perms::owner_write,
                  fs::perm_options::add);
  auto edited = niminal::app::edit_text_externally("original draft", script.string());
  if (edited != "edited draft") {
    std::cerr << "external editor did not return the edited buffer: " << edited << "\n";
    return 1;
  }
  {
    std::ofstream out(script, std::ios::trunc);
    out << "#!/bin/sh\nexit 3\n";
  }
  try {
    niminal::app::edit_text_externally("original draft", script.string());
    std::cerr << "external editor failure should throw\n";
    return 1;
  } catch (const std::exception&) {
  }

  fs::remove_all(root);
  return 0;
}
