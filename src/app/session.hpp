#pragma once

#include <niminal/types.hpp>
#include <nlohmann/json.hpp>

#include <filesystem>
#include <string>
#include <string_view>
#include <vector>

namespace niminal {
struct Agent;
}

namespace niminal::app {

struct SessionInfo {
  std::string id;
  std::string name;
  std::string preview;
  std::string workspace;
  std::filesystem::file_time_type mtime{};
  bool deleted = false;
};

class Session {
public:
  std::string id;
  std::string path;
  std::string workspace;
  std::string parent;
  std::string name;
  std::vector<nlohmann::json> events;
  bool persist = true;

  void add_user(const niminal::UserInput& input);
  void add_bash(const std::string& command, const std::string& output, bool exclude_from_context);
  void add_assistant(const std::string& text, const nlohmann::json& tool_calls,
                     const std::string& model, const niminal::Usage& usage = {},
                     const std::string& reasoning_content = {},
                     const nlohmann::json& reasoning_details = nlohmann::json::array());
  niminal::Usage usage_totals() const;
  void add_tool_result(const std::string& tool_id, const niminal::ToolResult& output,
                       bool is_error);
  void add_name(const std::string& title);
  void add_selection(const std::string& model, const std::string& provider = {});
  void add_extension(const std::string& extension, const nlohmann::json& data);
  void add_extension_message(const nlohmann::json& message);
  void add_compaction(const std::string& summary, int first_kept_index, int tokens_before,
                      const nlohmann::json& details = {});
  int recover_interrupted_tools();
  nlohmann::json openai_messages() const;
  std::string last_model() const;
  std::string last_provider() const;
  std::string last_assistant_text() const;
  std::string describe() const;
  int latest_compaction_index() const;
  int end_after_user_turn(int turn) const;
  std::vector<std::pair<int, std::string>> user_turn_previews() const;
  Session fork(const std::filesystem::path& dir, int upto = -1) const;
  std::string export_text(std::string_view format) const;

private:
  std::string damaged_;
  std::string valid_prefix_;
  bool needs_newline_ = false;
  void append(const nlohmann::json& event);
  friend Session load_session(const std::filesystem::path& dir, const std::string& id);
};

bool valid_session_id(std::string_view id);
std::filesystem::path default_session_dir();
Session create_session(const std::filesystem::path& dir, const std::string& workspace);
Session load_session(const std::filesystem::path& dir, const std::string& id);
std::vector<SessionInfo> list_sessions(const std::filesystem::path& dir,
                                       const std::string& workspace, int limit = 20);
std::vector<SessionInfo> search_sessions(const std::filesystem::path& dir,
                                         const std::string& workspace, const std::string& query,
                                         int limit = 20);
std::vector<SessionInfo> list_deleted_sessions(const std::filesystem::path& dir);
bool delete_session(const std::filesystem::path& dir, const std::string& id);
bool restore_session(const std::filesystem::path& dir, const std::string& id);
std::string format_session_list(const std::vector<SessionInfo>& infos,
                                const std::string& current_id,
                                std::string_view heading = "Sessions (newest first)");
void bind_session(niminal::Agent& agent, Session& session);

std::string serialize_session_event(const nlohmann::json& event);
int estimate_session_event_tokens(const nlohmann::json& event);

} // namespace niminal::app
