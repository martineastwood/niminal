#include "extensions.hpp"
#include "session.hpp"
#include "trust.hpp"

#include <niminal/http.hpp>
#include <niminal/openai.hpp>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <future>
#include <iostream>
#include <optional>
#include <stdexcept>
#include <thread>

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
  const fs::path examples = fs::path(NIMINAL_SOURCE_DIR) / "examples" / "extensions";
  for (const auto* name : {"powerline_footer", "todo_widget", "subagent_panel"}) {
    fs::copy(examples / name, root / ".niminal" / "extensions" / name, fs::copy_options::recursive);
  }
  setenv("HOME", home.c_str(), 1);
  niminal::app::set_project_resources_trusted(root, true);

  {
    std::ofstream out(dir / "extension.json");
    out << R"({"name":"fixture","command":["./extension.py"]})";
  }
  {
    std::ofstream out(dir / "extension.py");
    out << R"PY(#!/usr/bin/env python3
import json, os, sys
def send(value):
    print(json.dumps(value), flush=True)
send({"type":"register",
      "commands":[{"name":"hello","description":"Say hello"}],
      "tools":[{"name":"ext_echo","description":"Echo text",
                "input_schema":{"type":"object"},
                "capabilities":["read"]}],
      "events":["tool_call","tool_result","context","session_start",
                "session_before_compact","session_compact","turn_start","turn_end",
                "input","before_agent_start","session_shutdown","session_before_switch",
                "before_provider_headers","before_provider_request",
                "after_provider_response","agent_settled","message_end",
                "session_compact_failed"]})
for line in sys.stdin:
    message = json.loads(line)
    kind = message.get("type")
    if kind == "shutdown":
        break
    if kind == "initialize":
        continue
    reply = {"type":"response", "id":message.get("id", "")}
    if kind == "command":
        reply["message"] = "Hello " + message.get("arguments", "") + \
            " env=" + os.environ.get("NIMINAL_SESSION_ID", "none")
        reply["notification"] = {"level":"info", "message":"command ran"}
        reply["status"] = {"key":"state", "segments":[
            {"text":"ready", "style":"success"}]}
        reply["widget"] = {"key":"work", "content":[
            {"type":"text", "text":"extension widget", "style":"muted"}]}
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
    elif kind == "event" and message.get("event") == "input":
        reply["text"] = message["payload"]["text"] + " transformed"
    elif kind == "event" and message.get("event") == "before_agent_start":
        reply["system_prompt"] = "Task system"
        reply["message"] = {"content":"Persistent extension context"}
    elif kind == "event" and message.get("event") == "session_before_switch":
        reply["allow"] = False
        reply["reason"] = "unsaved work"
    elif kind == "event" and message.get("event") == "before_provider_headers":
        reply["headers"] = {"Authorization":None,"x-test":"yes"}
    elif kind == "event" and message.get("event") == "before_provider_request":
        reply["payload"] = {"model":"replacement"}
    elif kind == "event" and message.get("event") == "message_end":
        reply["text"] = "rewritten answer"
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
  auto env_dir = root / ".niminal" / "tools" / "env_dump";
  fs::create_directories(env_dir);
  {
    std::ofstream out(env_dir / "tool.json");
    out << R"({"name":"env_dump","description":"Dump session env","command":["./dump"],"input_schema":{"type":"object"},"capabilities":["read"]})";
  }
  {
    std::ofstream out(env_dir / "dump");
    out << "#!/bin/sh\necho \"$NIMINAL_SESSION_ID|$NIMINAL_MODEL\"\n";
  }
  fs::permissions(env_dir / "dump",
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
  // An inherited session block must not shadow the current session's values.
  setenv("NIMINAL_SESSION_ID", "inherited", 1);
  niminal::app::ShellEnvFn env_fn = [] {
    return niminal::app::ShellEnv{{"NIMINAL_SESSION_ID", "sess-7"},
                                  {"NIMINAL_SESSION_FILE", "s.jsonl"},
                                  {"NIMINAL_PROVIDER", "test"},
                                  {"NIMINAL_MODEL", "test/model"},
                                  {"NIMINAL_REASONING_LEVEL", "high"}};
  };
  auto runtime = ExtensionRuntime::start(root, "session", &cancel, &env_fn);
  if (runtime->commands().size() != 6) {
    std::cerr << "extension registration failed, got " << runtime->commands().size()
              << " commands\n";
    for (const auto& warning : runtime->warnings()) {
      std::cerr << warning << '\n';
    }
    return 1;
  }
  auto command = runtime->invoke("hello", "world");
  if (command.value("message", "") != "Hello world env=sess-7") {
    std::cerr << "extension process should receive the session env: "
              << command.value("message", "") << '\n';
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
  auto footer_demo = runtime->invoke("footer_demo", "");
  auto empty_todos = runtime->invoke("todos", "");
  auto subagents_demo = runtime->invoke("subagents_demo", "");
  auto find_widget = [&](const std::string& extension, const std::string& key) {
    auto widgets = runtime->widgets();
    const auto found = std::find_if(widgets.begin(), widgets.end(), [&](const auto& widget) {
      return widget.extension == extension && widget.key == key;
    });
    return found == widgets.end() ? std::optional<niminal::app::ExtensionWidget>() : *found;
  };
  auto find_status = [&](const std::string& extension, const std::string& key) {
    auto statuses = runtime->statuses();
    const auto found = std::find_if(statuses.begin(), statuses.end(), [&](const auto& status) {
      return status.extension == extension && status.key == key;
    });
    return found == statuses.end() ? std::optional<niminal::app::ExtensionStatus>() : *found;
  };
  const auto footer_status = find_status("powerline_footer", "model");
  const auto subagent_widget = find_widget("subagent_panel", "workers");
  if (footer_demo.value("message", "") != "Footer status updated." || !footer_status ||
      footer_status->segments.size() != 4 || footer_status->segments[0].style != "emphasis" ||
      empty_todos.value("message", "").find("No todos yet.") == std::string::npos ||
      subagents_demo.value("message", "") != "Showing simulated subagent activity." ||
      !subagent_widget || subagent_widget->actions.size() != 2) {
    std::cerr << "extension UI examples did not register their status and widgets\n";
    return 1;
  }
  auto tools = runtime->tools();
  niminal::Tool* todo_tool = nullptr;
  for (auto& tool : tools) {
    if (tool.name == "todo") {
      todo_tool = &tool;
    }
  }
  if (todo_tool == nullptr || todo_tool->read_only || !todo_tool->extension ||
      todo_tool->parameters["properties"]["action"]["enum"].size() != 6) {
    std::cerr << "todo tool was not registered for the agent\n";
    return 1;
  }
  const auto created_first = todo_tool->run(
      nlohmann::json{{"action", "create"}, {"subject", "Review the existing behavior"}});
  const auto created_second =
      todo_tool->run(nlohmann::json{{"action", "create"},
                                    {"subject", "Add the implementation"},
                                    {"description", "Include a regression test."}});
  const auto started_first =
      todo_tool->run(nlohmann::json{{"action", "update"},
                                    {"id", 1},
                                    {"status", "in_progress"},
                                    {"activeForm", "reviewing existing behavior"}});
  const auto todo_widget = find_widget("todo_widget", "tasks");
  const auto todo_list = runtime->invoke("todos", "");
  if (created_first.text.find("Created [pending] #1") == std::string::npos ||
      created_second.text.find("Created [pending] #2") == std::string::npos ||
      created_second.text.find("Current todo list:\nPending:") == std::string::npos ||
      started_first.text.find("in_progress") == std::string::npos || !todo_widget ||
      todo_widget->content.size() != 2 || todo_widget->content[0]["items"].size() != 2 ||
      todo_widget->content[0]["items"][0]["state"] != "active" ||
      todo_widget->actions.size() != 2 ||
      todo_list.value("message", "").find("Review the existing behavior") == std::string::npos) {
    std::cerr << "todo tool did not create an agent-visible task list\n";
    return 1;
  }
  if (!runtime->activate_widget_action("todo_widget", "tasks", "complete:1") ||
      runtime->activate_widget_action("todo_widget", "tasks", "missing")) {
    std::cerr << "todo widget action dispatch validation failed\n";
    return 1;
  }
  const auto todo_deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
  bool todo_updated = false;
  while (std::chrono::steady_clock::now() < todo_deadline) {
    runtime->pump();
    const auto updated = find_widget("todo_widget", "tasks");
    if (updated && std::any_of(updated->content[0]["items"].begin(),
                               updated->content[0]["items"].end(), [](const auto& item) {
                                 return item.value("text", "").starts_with("#1 ") &&
                                        item.value("state", "") == "done";
                               })) {
      todo_updated = true;
      break;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(10));
  }
  if (!todo_updated || !runtime->activate_widget_action("subagent_panel", "workers", "stop")) {
    std::cerr << "widget action did not update the extension view\n";
    return 1;
  }
  const auto subagent_deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
  bool subagent_stopped = false;
  while (std::chrono::steady_clock::now() < subagent_deadline) {
    runtime->pump();
    const auto updated = find_widget("subagent_panel", "workers");
    if (updated && updated->content[0]["items"][1]["state"] == "done" &&
        updated->content[0]["items"][1]["text"] == "Review extension protocol · stopped") {
      subagent_stopped = true;
      break;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(10));
  }
  if (!subagent_stopped) {
    std::cerr << "subagent demo action did not update its widget\n";
    return 1;
  }
  auto clear_footer = runtime->invoke("footer_demo", "clear");
  if (clear_footer.value("message", "") != "Footer status updated." ||
      find_status("powerline_footer", "model")) {
    std::cerr << "empty status segments should clear the footer entry\n";
    return 1;
  }
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
    persistent_output = persistent->run(nlohmann::json::object()).text;
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
      external->run(nlohmann::json{{"value", 1}}).text.find("\"value\":1") == std::string::npos) {
    return 1;
  }
  niminal::Tool* env_dump = nullptr;
  for (auto& tool : tools) {
    if (tool.name == "env_dump") {
      env_dump = &tool;
    }
  }
  if (env_dump == nullptr ||
      env_dump->run(nlohmann::json::object()).text.find("sess-7|test/model") == std::string::npos) {
    std::cerr << "external tool should receive the session env\n";
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
  const auto fixture_status = find_status("fixture", "state");
  const auto fixture_widget = find_widget("fixture", "work");
  if (!fixture_status || fixture_status->segments.size() != 1 ||
      fixture_status->segments[0].text != "ready" || !fixture_widget ||
      fixture_widget->content.size() != 1 ||
      fixture_widget->content[0].value("text", "") != "extension widget") {
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
  auto blocked_switch =
      runtime->dispatch(HookEvent::session_before_switch, nlohmann::json{{"reason", "new"}});
  if (blocked_switch.allowed || blocked_switch.reason != "unsaved work") {
    return 1;
  }
  auto headers = runtime->dispatch(HookEvent::before_provider_headers,
                                   nlohmann::json{{"headers", {{"Authorization", "Bearer test"}}}});
  if (!headers.has_headers || headers.headers.contains("Authorization") ||
      headers.headers.value("x-test", "") != "yes") {
    return 1;
  }
  auto payload = runtime->dispatch(HookEvent::before_provider_request,
                                   nlohmann::json{{"payload", {{"model", "original"}}}});
  if (!payload.has_payload || payload.payload.value("model", "") != "replacement") {
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
  niminal::app::bind_session(agent, session);
  agent.messages = session.openai_messages();
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
  niminal::ChatRequest captured;
  agent.system = "Original system";
  agent.stream_chat_fn = [&](const niminal::ChatRequest& request) {
    captured = request;
    niminal::ChatResult result;
    result.text = "original answer";
    return result;
  };
  if (agent.run("question") != "rewritten answer" ||
      captured.messages[0]["content"][0].value("text", "") != "Task system" ||
      captured.messages[2].value("content", "") != "question transformed" ||
      captured.messages[3].value("content", "") != "Persistent extension context" ||
      session.events.back().value("type", "") != "assistant" ||
      session.events[session.events.size() - 2].value("type", "") != "extension_message" ||
      session.openai_messages()[2].value("content", "") != "Persistent extension context") {
    std::cerr << "agent hook integration failed: " << captured.messages.dump() << "\n"
              << session.openai_messages().dump() << "\n";
    return 1;
  }
  if (!captured.before_provider_request || !captured.before_provider_headers ||
      !captured.after_provider_response) {
    return 1;
  }
  nlohmann::json provider_payload{{"model", "original"}};
  captured.before_provider_request(provider_payload);
  std::map<std::string, std::string> provider_headers{{"Authorization", "Bearer test"}};
  captured.before_provider_headers(provider_headers);
  niminal::HttpResponse denied;
  denied.status = 403;
  denied.body = "denied";
  captured.after_provider_response(denied);
  if (provider_payload.value("model", "") != "replacement" ||
      provider_headers.contains("Authorization") || provider_headers["x-test"] != "yes") {
    std::cerr << "provider hook integration failed\n";
    return 1;
  }
  runtime->dispatch(HookEvent::session_shutdown, nlohmann::json{{"reason", "quit"}});
  runtime->dispatch(HookEvent::session_compact_failed, nlohmann::json{{"error", "test"}});
  runtime->stop();

  runtime = ExtensionRuntime::start(root, "session", &cancel, &env_fn);
  runtime->dispatch(HookEvent::session_start, niminal::app::session_hook_payload("session", root));
  const auto restored_todos = find_widget("todo_widget", "tasks");
  auto restored_tools = runtime->tools();
  niminal::Tool* restored_todo_tool = nullptr;
  for (auto& tool : restored_tools) {
    if (tool.name == "todo") {
      restored_todo_tool = &tool;
    }
  }
  const bool has_restored_items = restored_todos && !restored_todos->content.empty() &&
                                  restored_todos->content[0].contains("items") &&
                                  restored_todos->content[0]["items"].is_array();
  const bool first_task_completed =
      has_restored_items &&
      std::any_of(restored_todos->content[0]["items"].begin(),
                  restored_todos->content[0]["items"].end(), [](const auto& item) {
                    return item.value("text", "").starts_with("#1 ") &&
                           item.value("state", "") == "done";
                  });
  if (!has_restored_items || restored_todos->content[0]["items"].size() != 2 ||
      !first_task_completed || restored_todo_tool == nullptr ||
      restored_todo_tool->run(nlohmann::json{{"action", "list"}})
              .text.find("Add the implementation") == std::string::npos) {
    std::cerr << "todo tasks did not survive an extension restart\n";
    return 1;
  }
  const auto cleared = restored_todo_tool->run(nlohmann::json{{"action", "clear"}});
  if (!cleared.text.starts_with("Cleared 2 tasks.") ||
      runtime->invoke("todos", "").value("message", "") != "No todos yet. Ask me to add tasks.") {
    std::cerr << "todo clear did not remove the saved task list\n";
    return 1;
  }
  runtime->stop();

  // Cancelling a turn must not report the aborted extension requests as failures.
  runtime = ExtensionRuntime::start(root, "session", &cancel, &env_fn);
  cancel = true;
  const auto cancelled =
      runtime->dispatch(HookEvent::turn_end, nlohmann::json{{"interrupted", true}});
  if (!cancelled.warnings.empty()) {
    std::cerr << "cancelled turn reported extension warning: " << cancelled.warnings.front()
              << '\n';
    return 1;
  }
  bool cancelled_request = false;
  try {
    runtime->invoke("hello", "world");
  } catch (const niminal::Cancelled&) {
    cancelled_request = true;
  } catch (const std::exception& e) {
    std::cerr << "cancelled request threw: " << e.what() << '\n';
    return 1;
  }
  cancel = false;
  if (!cancelled_request) {
    std::cerr << "cancelled request did not report cancellation\n";
    return 1;
  }
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
