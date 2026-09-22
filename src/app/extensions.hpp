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
  nlohmann::json arguments;
  bool has_arguments = false;
  std::string output;
  bool has_output = false;
  bool is_error = false;
  bool has_is_error = false;
  std::vector<std::string> system;
  nlohmann::json messages = nlohmann::json::array();
  std::string instruction;
  bool has_compaction = false;
  std::string summary;
  int first_kept_index = 0;
  nlohmann::json details;
  std::string text;
  bool has_text = false;
  std::string system_prompt;
  bool has_system_prompt = false;
  nlohmann::json headers;
  bool has_headers = false;
  nlohmann::json payload;
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
  nlohmann::json data;
};

std::string edit_text_externally(const std::string& text, const std::string& editor);

class ExtensionRuntime : public std::enable_shared_from_this<ExtensionRuntime> {
  struct Access {};

public:
  struct Impl;
  static std::shared_ptr<ExtensionRuntime> start(const std::filesystem::path& workspace,
                                                 const std::string& session_id,
                                                 std::atomic<bool>* cancel = nullptr,
                                                 const ShellEnvFn* shell_env = nullptr);
  ~ExtensionRuntime();

  ExtensionRuntime(const ExtensionRuntime&) = delete;
  ExtensionRuntime& operator=(const ExtensionRuntime&) = delete;

  const std::vector<ExtensionCommand>& commands() const { return commands_; }
  const std::vector<std::string>& warnings() const { return warnings_; }
  std::vector<niminal::Tool> tools();
  nlohmann::json invoke(const std::string& name, const std::string& arguments,
                        const nlohmann::json& context = {});
  void set_ui_callbacks(ExtensionUiCallbacks callbacks);
  std::string edit_text(const std::string& title, const std::string& text);
  void set_tool_update(std::function<void(const std::string&, const std::string&)> callback);
  void set_host_request(
      std::function<nlohmann::json(const std::string&, const nlohmann::json&)> callback);
  HookOutcome dispatch(HookEvent event, const nlohmann::json& payload);
  bool pump();
  void stop();

  std::vector<ExtensionNotice> take_notices();
  std::vector<ExtensionUserMessage> take_user_messages();
  std::vector<ExtensionEntry> take_entries();
  std::vector<std::string> status_texts() const;
  std::vector<std::string> widget_lines() const;

  explicit ExtensionRuntime(Access, std::filesystem::path workspace, std::atomic<bool>* cancel);

private:
  std::unique_ptr<Impl> impl_;
  std::filesystem::path workspace_;
  std::atomic<bool>* cancel_ = nullptr;
  std::vector<ExtensionCommand> commands_;
  std::vector<std::string> warnings_;
};

nlohmann::json session_hook_payload(const std::string& session_id,
                                    const std::filesystem::path& workspace);
void bind_extensions(niminal::Agent& agent, const std::shared_ptr<ExtensionRuntime>& runtime,
                     const std::filesystem::path& workspace,
                     const std::function<void(const std::string&)>& note = {},
                     Session* session = nullptr, const Config& cfg = {});
void install_extension_tools(niminal::Agent& agent,
                             const std::shared_ptr<ExtensionRuntime>& runtime,
                             const std::vector<std::string>* allowed = nullptr);

} // namespace niminal::app
