#pragma once

#include <niminal/agent.hpp>

#include <atomic>
#include <filesystem>
#include <functional>
#include <memory>
#include <string>
#include <vector>

namespace niminal::app {

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

class ExtensionRuntime : public std::enable_shared_from_this<ExtensionRuntime> {
 public:
  struct Impl;
  static std::shared_ptr<ExtensionRuntime> start(
      const std::filesystem::path& workspace, const std::string& session_id,
      std::atomic<bool>* cancel = nullptr);
  ~ExtensionRuntime();

  ExtensionRuntime(const ExtensionRuntime&) = delete;
  ExtensionRuntime& operator=(const ExtensionRuntime&) = delete;

  const std::vector<ExtensionCommand>& commands() const { return commands_; }
  const std::vector<std::string>& warnings() const { return warnings_; }
  std::vector<niminal::Tool> tools();
  nlohmann::json invoke(const std::string& name, const std::string& arguments,
                        const nlohmann::json& context = {});
  HookOutcome dispatch(HookEvent event, const nlohmann::json& payload);
  void pump();
  void stop();

  std::vector<ExtensionNotice> take_notices();
  std::vector<ExtensionUserMessage> take_user_messages();
  std::vector<ExtensionEntry> take_entries();
  std::vector<std::string> status_texts() const;
  std::vector<std::string> widget_lines() const;

 private:
  explicit ExtensionRuntime(std::filesystem::path workspace,
                            std::atomic<bool>* cancel);
  std::unique_ptr<Impl> impl_;
  std::filesystem::path workspace_;
  std::atomic<bool>* cancel_ = nullptr;
  std::vector<ExtensionCommand> commands_;
  std::vector<std::string> warnings_;
};

nlohmann::json session_hook_payload(const std::string& session_id,
                                    const std::filesystem::path& workspace);
void bind_extensions(niminal::Agent& agent,
                     const std::shared_ptr<ExtensionRuntime>& runtime,
                     const std::filesystem::path& workspace,
                     std::function<void(const std::string&)> note = {});
void install_extension_tools(
    niminal::Agent& agent,
    const std::shared_ptr<ExtensionRuntime>& runtime);

}  // namespace niminal::app
