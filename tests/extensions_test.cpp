#include "extensions.hpp"
#include "session.hpp"
#include "trust.hpp"

#include <niminal/chat.hpp>

#include <algorithm>
#include <atomic>
#include <cerrno>
#include <chrono>
#include <csignal>
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
  auto tool_dir = root / ".niminal" / "tools" / "echo_json";
  auto broken_tool_dir = root / ".niminal" / "tools" / "broken";
  auto invalid_capability_dir = root / ".niminal" / "tools" / "invalid_capability";
  auto collision_tool_dir = root / ".niminal" / "tools" / "collision";
  fs::remove_all(root);
  fs::create_directories(home);
  fs::create_directories(tool_dir);
  fs::create_directories(broken_tool_dir);
  fs::create_directories(invalid_capability_dir);
  fs::create_directories(collision_tool_dir);
  const fs::path fixtures = fs::path(NIMINAL_EXTENSIONS_FIXTURES_DIR);
  fs::create_directories(root / ".niminal" / "extensions");
  for (const auto* name : {"fixture", "host", "orphan_guard", "parallel", "status_demo",
                           "stdin_watcher", "todo_demo", "widget_demo"}) {
    fs::copy(fixtures / name, root / ".niminal" / "extensions" / name, fs::copy_options::recursive);
  }
  setenv("HOME", home.c_str(), 1);
  niminal::app::set_project_resources_trusted(root, true);
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
    std::ofstream out(invalid_capability_dir / "tool.json");
    out << R"({"name":"invalid_capability","description":"Invalid capability","command":["./run"],"input_schema":{"type":"object"},"capabilities":["write","unknown"]})";
  }
  {
    std::ofstream out(collision_tool_dir / "tool.json");
    out << R"({"name":"bash","description":"Collision","command":["./run"],"input_schema":{"type":"object"}})";
  }
  niminal::Cancellation cancel;
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
  const auto names = runtime->names();
  if (std::find(names.begin(), names.end(), "fixture") == names.end()) {
    std::cerr << "runtime should report loaded extension names\n";
    return 1;
  }
  const auto tool_names = runtime->tool_names();
  if (std::find(tool_names.begin(), tool_names.end(), "echo_json") == tool_names.end()) {
    std::cerr << "runtime should report loaded external tool names\n";
    return 1;
  }
  auto command = runtime->invoke("hello", "world");
  if (niminal::json_value(command, "message", "") != "Hello world env=sess-7") {
    std::cerr << "extension process should receive the session env: "
              << niminal::json_value(command, "message", "") << '\n';
    return 1;
  }
  runtime->set_host_request([](const std::string& method, const niminal::json& request) {
    if (method != "model.complete" || niminal::json_value(request, "max_tokens", 0) != 123) {
      throw std::runtime_error("unexpected model request");
    }
    return niminal::json{{"text", "generated"}, {"model", "test/model"}, {"finish_reason", "stop"}};
  });
  auto model_host = runtime->invoke("host", "model");
  if (niminal::json_value(model_host["payload"]["model"]["result"], "text", "") != "generated" ||
      niminal::json_value(model_host["payload"]["model"]["result"], "model", "") != "test/model") {
    return 1;
  }
  auto first = std::async(std::launch::async, [&] { return runtime->invoke("parallel", "one"); });
  auto second = std::async(std::launch::async, [&] { return runtime->invoke("parallel", "two"); });
  auto first_result = first.get();
  auto second_result = second.get();
  if (niminal::json_value(first_result, "message", "") != "first" ||
      niminal::json_value(second_result, "message", "") != "second") {
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
  const auto footer_status = find_status("status_demo", "model");
  const auto subagent_widget = find_widget("widget_demo", "workers");
  if (niminal::json_value(footer_demo, "message", "") != "Footer status updated." ||
      !footer_status || footer_status->segments.size() != 4 ||
      footer_status->segments[0].style != "emphasis" ||
      niminal::json_value(empty_todos, "message", "").find("No todos yet.") == std::string::npos ||
      niminal::json_value(subagents_demo, "message", "") !=
          "Showing simulated subagent activity." ||
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
      niminal::json{{"action", "create"}, {"subject", "Review the existing behavior"}});
  const auto created_second =
      todo_tool->run(niminal::json{{"action", "create"},
                                   {"subject", "Add the implementation"},
                                   {"description", "Include a regression test."}});
  const auto started_first =
      todo_tool->run(niminal::json{{"action", "update"},
                                   {"id", 1},
                                   {"status", "in_progress"},
                                   {"activeForm", "reviewing existing behavior"}});
  const auto todo_widget = find_widget("todo_demo", "tasks");
  const auto todo_list = runtime->invoke("todos", "");
  if (created_first.text.find("Created [pending] #1") == std::string::npos ||
      created_second.text.find("Created [pending] #2") == std::string::npos ||
      created_second.text.find("Current todo list:\nPending:") == std::string::npos ||
      started_first.text.find("in_progress") == std::string::npos || !todo_widget ||
      todo_widget->content.size() != 2 || todo_widget->content[0]["items"].size() != 2 ||
      niminal::json_value(todo_widget->content[0]["items"][0], "state", "") != "active" ||
      todo_widget->actions.size() != 2 ||
      niminal::json_value(todo_list, "message", "").find("Review the existing behavior") ==
          std::string::npos) {
    std::cerr << "todo tool did not create an agent-visible task list\n";
    return 1;
  }
  if (!runtime->activate_widget_action("todo_demo", "tasks", "complete:1") ||
      runtime->activate_widget_action("todo_demo", "tasks", "missing")) {
    std::cerr << "todo widget action dispatch validation failed\n";
    return 1;
  }
  const auto todo_deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
  bool todo_updated = false;
  while (std::chrono::steady_clock::now() < todo_deadline) {
    runtime->pump();
    const auto updated = find_widget("todo_demo", "tasks");
    if (updated &&
        std::any_of(updated->content[0]["items"].get_array().begin(),
                    updated->content[0]["items"].get_array().end(), [](const auto& item) {
                      return niminal::json_value(item, "text", "").starts_with("#1 ") &&
                             niminal::json_value(item, "state", "") == "done";
                    })) {
      todo_updated = true;
      break;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(10));
  }
  if (!todo_updated || !runtime->activate_widget_action("widget_demo", "workers", "stop")) {
    std::cerr << "widget action did not update the extension view\n";
    return 1;
  }
  const auto subagent_deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
  bool subagent_stopped = false;
  while (std::chrono::steady_clock::now() < subagent_deadline) {
    runtime->pump();
    const auto updated = find_widget("widget_demo", "workers");
    if (updated && niminal::json_value(updated->content[0]["items"][1], "state", "") == "done" &&
        niminal::json_value(updated->content[0]["items"][1], "text", "") ==
            "Second task · stopped") {
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
  if (niminal::json_value(clear_footer, "message", "") != "Footer status updated." ||
      find_status("status_demo", "model")) {
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
    persistent_output = persistent->run(niminal::json_object()).text;
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
      external->run(niminal::json{{"value", 1}}).text.find("\"value\":1") == std::string::npos) {
    return 1;
  }
  niminal::Tool* env_dump = nullptr;
  for (auto& tool : tools) {
    if (tool.name == "env_dump") {
      env_dump = &tool;
    }
  }
  if (env_dump == nullptr ||
      env_dump->run(niminal::json_object()).text.find("sess-7|test/model") == std::string::npos) {
    std::cerr << "external tool should receive the session env\n";
    return 1;
  }
  bool warned_broken = false;
  bool warned_capability = false;
  bool warned_collision = false;
  for (const auto& warning : runtime->warnings()) {
    warned_broken = warned_broken || warning.find("broken") != std::string::npos;
    warned_capability =
        warned_capability || warning.find("unknown capability: unknown") != std::string::npos;
    warned_collision = warned_collision || warning.find("bash") != std::string::npos;
  }
  if (!warned_broken || !warned_capability || !warned_collision ||
      std::any_of(tools.begin(), tools.end(),
                  [](const auto& tool) { return tool.name == "invalid_capability"; })) {
    return 1;
  }

  auto pre =
      runtime->dispatch(HookEvent::tool_call,
                        niminal::json{{"tool", "bash"}, {"arguments", {{"command", "original"}}}});
  if (!pre.has_arguments || niminal::json_value(pre.arguments, "command", "") != "changed") {
    return 1;
  }
  auto post =
      runtime->dispatch(HookEvent::tool_result, niminal::json{{"tool", "bash"},
                                                              {"arguments", niminal::json_object()},
                                                              {"output", "original"},
                                                              {"is_error", false}});
  if (!post.has_output || post.output != "rewritten" || !post.has_is_error || !post.is_error) {
    return 1;
  }
  auto context = runtime->dispatch(HookEvent::context, niminal::json_object());
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
      niminal::json_value(fixture_widget->content[0], "text", "") != "extension widget") {
    return 1;
  }
  auto entries = runtime->take_entries();
  auto user_messages = runtime->take_user_messages();
  if (entries.size() != 1 || niminal::json_value(entries[0].data, "count", 0) != 1 ||
      user_messages.size() != 1 || user_messages[0].content != "background done") {
    return 1;
  }
  auto compact = runtime->dispatch(HookEvent::session_before_compact, niminal::json_object());
  if (!compact.has_compaction || compact.summary != "extension summary" ||
      compact.first_kept_index != 1 ||
      niminal::json_value(compact.details, "source", "") != "fixture") {
    return 1;
  }
  auto blocked_switch =
      runtime->dispatch(HookEvent::session_before_switch, niminal::json{{"reason", "new"}});
  if (blocked_switch.allowed || blocked_switch.reason != "unsaved work") {
    return 1;
  }
  auto headers = runtime->dispatch(HookEvent::before_provider_headers,
                                   niminal::json{{"headers", {{"Authorization", "Bearer test"}}}});
  if (!headers.has_headers || headers.headers.contains("Authorization") ||
      niminal::json_value(headers.headers, "x-test", "") != "yes") {
    return 1;
  }
  auto payload = runtime->dispatch(HookEvent::before_provider_request,
                                   niminal::json{{"payload", {{"model", "original"}}}});
  if (!payload.has_payload || niminal::json_value(payload.payload, "model", "") != "replacement") {
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
  niminal::app::Config cfg;
  niminal::app::bind_extensions(agent, runtime, root, cfg, {}, &session);
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
  if (ui_host["payload"]["question"]["answer"].get<std::string>() != "Blue" ||
      !ui_host["payload"]["confirm"]["confirmed"].get<bool>() ||
      ui_host["payload"]["input"]["answer"].get<std::string>() != "feature" ||
      ui_host["payload"]["password"]["answer"].get<std::string>() != "secret-token") {
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
  if (niminal::json_value(host["payload"]["info"]["result"], "id", "") != "session" ||
      niminal::json_value(host["payload"]["info"]["result"], "event_count", 0) != 1 ||
      niminal::json_value(host["payload"]["name"]["result"], "name", "") != "handoff source" ||
      niminal::json_value(host["payload"]["usage"]["result"], "tokens", 0) <= 0 ||
      niminal::json_value(host["payload"]["usage"]["result"], "limit", 0) <= 0 ||
      niminal::json_value(host["payload"]["editor"]["result"], "text", "") != "edited handoff") {
    return 1;
  }
  niminal::json arguments{{"command", "original"}};
  std::string reason;
  niminal::ToolCall call{"tool", "bash", R"({"command":"original"})"};
  if (!agent.before_tool(call, arguments, reason) ||
      niminal::json_value(arguments, "command", "") != "changed") {
    return 1;
  }
  std::string output = "original";
  bool is_error = false;
  agent.after_tool(call, arguments, output, is_error);
  if (output != "rewritten" || !is_error) {
    return 1;
  }
  niminal::json messages = niminal::json_array(
      {{{"role", "system"},
        {"content", niminal::json_array({{{"type", "text"}, {"text", "Base"}}})}},
       {{"role", "user"}, {"content", "Original"}}});
  agent.augment_context(messages);
  if (messages.size() != 3 || messages[0]["content"].size() != 2 ||
      niminal::json_value(messages[2], "content", "") != "Injected context") {
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
      niminal::json_value(captured.messages[0]["content"][0], "text", "") != "Task system" ||
      niminal::json_value(captured.messages[2], "content", "") != "question transformed" ||
      niminal::json_value(captured.messages[3], "content", "") != "Persistent extension context" ||
      niminal::json_value(session.events.back(), "type", "") != "assistant" ||
      niminal::json_value(session.events[session.events.size() - 2], "type", "") !=
          "extension_message" ||
      niminal::json_value(session.openai_messages()[2], "content", "") !=
          "Persistent extension context") {
    std::cerr << "agent hook integration failed: " << niminal::json_dump(captured.messages) << "\n"
              << niminal::json_dump(session.openai_messages()) << "\n";
    return 1;
  }
  if (!captured.before_provider_request || !captured.before_provider_headers ||
      !captured.after_provider_response) {
    return 1;
  }
  niminal::json provider_payload{{"model", "original"}};
  captured.before_provider_request(provider_payload);
  std::map<std::string, std::string> provider_headers{{"Authorization", "Bearer test"}};
  captured.before_provider_headers(provider_headers);
  niminal::ProviderResponse denied;
  denied.status = 403;
  denied.body = "denied";
  captured.after_provider_response(denied);
  if (niminal::json_value(provider_payload, "model", "") != "replacement" ||
      provider_headers.contains("Authorization") || provider_headers["x-test"] != "yes") {
    std::cerr << "provider hook integration failed\n";
    return 1;
  }
  runtime->dispatch(HookEvent::session_shutdown, niminal::json{{"reason", "quit"}});
  runtime->dispatch(HookEvent::session_compact_failed, niminal::json{{"error", "test"}});
  runtime->stop();

  runtime = ExtensionRuntime::start(root, "session", &cancel, &env_fn);
  runtime->dispatch(HookEvent::session_start, niminal::app::session_hook_payload("session", root));
  const auto restored_todos = find_widget("todo_demo", "tasks");
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
      std::any_of(restored_todos->content[0]["items"].get_array().begin(),
                  restored_todos->content[0]["items"].get_array().end(), [](const auto& item) {
                    return niminal::json_value(item, "text", "").starts_with("#1 ") &&
                           niminal::json_value(item, "state", "") == "done";
                  });
  if (!has_restored_items || restored_todos->content[0]["items"].size() != 2 ||
      !first_task_completed || restored_todo_tool == nullptr ||
      restored_todo_tool->run(niminal::json{{"action", "list"}})
              .text.find("Add the implementation") == std::string::npos) {
    std::cerr << "todo tasks did not survive an extension restart\n";
    return 1;
  }
  const auto cleared = restored_todo_tool->run(niminal::json{{"action", "clear"}});
  if (!cleared.text.starts_with("Cleared 2 tasks.") ||
      niminal::json_value(runtime->invoke("todos", ""), "message", "") != "No todos yet.") {
    std::cerr << "todo clear did not remove the saved task list\n";
    return 1;
  }
  runtime->stop();

  // Cancelling a turn must not report the aborted extension requests as failures.
  runtime = ExtensionRuntime::start(root, "session", &cancel, &env_fn);
  cancel.request();
  const auto cancelled =
      runtime->dispatch(HookEvent::turn_end, niminal::json{{"interrupted", true}});
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
  cancel.clear();
  if (!cancelled_request) {
    std::cerr << "cancelled request did not report cancellation\n";
    return 1;
  }
  runtime->stop();

  // A cancellation request during startup must not abort the registration
  // handshake; extensions should still load.
  cancel.request();
  auto interrupted = ExtensionRuntime::start(root, "session", &cancel, &env_fn);
  cancel.clear();
  for (const auto& warning : interrupted->warnings()) {
    if (warning.contains("interrupted")) {
      std::cerr << "startup aborted by cancellation: " << warning << '\n';
      return 1;
    }
  }
  const auto interrupted_names = interrupted->names();
  if (std::find(interrupted_names.begin(), interrupted_names.end(), "fixture") ==
      interrupted_names.end()) {
    std::cerr << "startup under cancellation did not load extensions\n";
    return 1;
  }
  interrupted->stop();

  // Closing stdin tells an extension that niminal is gone. A sibling that
  // inherited the pipe would hide that and leave the extension running.
  auto eof_marker = root / "eof.marker";
  fs::remove(eof_marker);
  setenv("NIMINAL_EOF_MARKER", eof_marker.c_str(), 1);
  auto watcher = ExtensionRuntime::start(root, "session", &cancel);
  watcher->stop();
  unsetenv("NIMINAL_EOF_MARKER");
  if (!fs::exists(eof_marker)) {
    std::cerr << "extension did not see stdin close while siblings were running\n";
    return 1;
  }

  // An extension that stops its own process tree needs the stop grace: killing
  // the extension first orphans a child it put in its own session.
  auto guard_pid_file = root / "orphan.pid";
  fs::remove(guard_pid_file);
  setenv("NIMINAL_ORPHAN_PID_FILE", guard_pid_file.c_str(), 1);
  auto guard = ExtensionRuntime::start(root, "session", &cancel);
  int guard_pid = 0;
  for (int attempt = 0; attempt < 60 && guard_pid == 0; ++attempt) {
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    std::ifstream in(guard_pid_file);
    in >> guard_pid;
  }
  guard->stop();
  unsetenv("NIMINAL_ORPHAN_PID_FILE");
  if (guard_pid == 0 || kill(guard_pid, 0) == 0 || errno != ESRCH) {
    std::cerr << "extension child outlived the stop grace: " << guard_pid << '\n';
    return 1;
  }

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
