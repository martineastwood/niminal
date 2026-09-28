#pragma once

#include <niminal/agent.hpp>

#include "config.hpp"
#include "tools.hpp"

#include <atomic>
#include <filesystem>
#include <functional>
#include <memory>
#include <string>
#include <vector>

namespace niminal::app {

class Session;

struct ExtensionUiCallbacks {
  std::function<std::string(const std::string&, const std::vector<std::string>&)> question;
  std::function<std::string(const std::string&, bool)> input;
  std::function<std::string(const std::string&, const std::string&)> editor;
};

enum class HookEvent {
  tool_call,
  tool_result,
  session_start,
  session_end,
  session_before_compact,
  session_compact,
  turn_start,
  turn_end,
  context,
  before_agent_start,
  input,
  session_shutdown,
  session_before_switch,
  before_provider_headers,
  before_provider_request,
  after_provider_response,
  agent_settled,
  message_end,
  session_compact_failed,
};

const char* hook_event_name(HookEvent event);

struct HookOutcome {
  bool allowed = true;
  std::string reason;
  std::vector<std::string> warnings;
  niminal::json arguments;
  bool has_arguments = false;
  std::string output;
  bool has_output = false;
  bool is_error = false;
  bool has_is_error = false;
  std::vector<std::string> system;
  niminal::json messages = json_array();
  std::string instruction;
  bool has_compaction = false;
  std::string summary;
  int first_kept_index = 0;
  niminal::json details;
  std::string text;
  bool has_text = false;
  std::string system_prompt;
  bool has_system_prompt = false;
  niminal::json headers;
  bool has_headers = false;
  niminal::json payload;
  bool has_payload = false;
};

struct ExtensionCommand {
  std::string name;
  std::string description;
  size_t extension = 0;
};

struct ExtensionNotice {
  std::string level;
  std::string message;
};

struct ExtensionUserMessage {
  std::string content;
  std::string deliver_as;
};

struct ExtensionEntry {
  std::string extension;
  niminal::json data;
};

struct ExtensionStatusSegment {
  std::string text;
  std::string style;
};

struct ExtensionStatus {
  std::string extension;
  std::string key;
  std::vector<ExtensionStatusSegment> segments;
};

struct ExtensionUiAction {
  std::string id;
  std::string label;
};

struct ExtensionWidget {
  std::string extension;
  std::string key;
  std::string position;
  std::string title;
  niminal::json content = json_array();
  std::vector<ExtensionUiAction> actions;
};

std::string edit_text_externally(const std::string& text, const std::string& editor);

class ExtensionRuntime : public std::enable_shared_from_this<ExtensionRuntime> {
  struct Access {};

public:
  struct Impl;
  static std::shared_ptr<ExtensionRuntime> start(const std::filesystem::path& workspace,
                                                 const std::string& session_id,
                                                 niminal::Cancellation* cancel = nullptr,
                                                 const ShellEnvFn* shell_env = nullptr);
  ~ExtensionRuntime();

  ExtensionRuntime(const ExtensionRuntime&) = delete;
  ExtensionRuntime& operator=(const ExtensionRuntime&) = delete;

  const std::vector<ExtensionCommand>& commands() const { return commands_; }
  const std::vector<std::string>& warnings() const { return warnings_; }
  std::vector<std::string> names() const;
  std::vector<niminal::Tool> tools();
  niminal::json invoke(const std::string& name, const std::string& arguments,
                       const niminal::json& context = {});
  void set_ui_callbacks(ExtensionUiCallbacks callbacks);
  std::string edit_text(const std::string& title, const std::string& text);
  void set_tool_update(std::function<void(const std::string&, const std::string&)> callback);
  void
  set_host_request(std::function<niminal::json(const std::string&, const niminal::json&)> callback);
  HookOutcome dispatch(HookEvent event, const niminal::json& payload);
  bool pump();
  void stop();

  std::vector<ExtensionNotice> take_notices();
  std::vector<ExtensionUserMessage> take_user_messages();
  std::vector<ExtensionEntry> take_entries();
  std::vector<ExtensionStatus> statuses() const;
  std::vector<ExtensionWidget> widgets() const;
  bool activate_widget_action(const std::string& extension, const std::string& key,
                              const std::string& action);

  explicit ExtensionRuntime(Access, std::filesystem::path workspace, niminal::Cancellation* cancel);

private:
  std::unique_ptr<Impl> impl_;
  std::filesystem::path workspace_;
  niminal::Cancellation* cancel_ = nullptr;
  std::vector<ExtensionCommand> commands_;
  std::vector<std::string> warnings_;
};

niminal::json session_hook_payload(const std::string& session_id,
                                   const std::filesystem::path& workspace);
void bind_extensions(niminal::Agent& agent, const std::shared_ptr<ExtensionRuntime>& runtime,
                     const std::filesystem::path& workspace, const Config& cfg,
                     const std::function<void(const std::string&)>& note = {},
                     Session* session = nullptr);
void install_extension_tools(niminal::Agent& agent,
                             const std::shared_ptr<ExtensionRuntime>& runtime,
                             const std::vector<std::string>* allowed = nullptr);

} // namespace niminal::app
