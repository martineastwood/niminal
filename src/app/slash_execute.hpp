#pragma once

#include "transcript.hpp"

#include <atomic>
#include <filesystem>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace niminal {
struct Agent;
struct UserInput;
} // namespace niminal

namespace niminal::app {

class ExtensionRuntime;
class Keybindings;
class PermissionPolicy;
class Session;
struct Config;
struct Theme;

struct SlashHost {
  niminal::Agent& agent;
  Session& session;
  Config& cfg;
  Theme& theme;
  PermissionPolicy& permissions;
  std::filesystem::path cwd;
  const Keybindings& keybindings;
  std::shared_ptr<ExtensionRuntime>& extensions;
  std::atomic<bool>& busy;
  bool& yolo_mode;
  bool& retry_available;
  niminal::UserInput& retry_prompt;
  bool& settings_open;
  int& settings_i;
  std::optional<std::string>& settings_edit;
  std::string& settings_error;
  std::vector<Block>& blocks;
  std::vector<std::pair<std::string, std::string>>& pending_changes;

  std::function<void(const std::string&)> flash_footer;
  std::function<bool(const std::string& reason, const std::string& target)> allow_session_switch;
  std::function<void(Session next, const std::string& note, const std::string& reason)>
      adopt_session;
  std::function<void()> restart_extensions;
  std::function<void()> reload_local;
  std::function<void()> apply_extension_actions;
  std::function<void(niminal::UserInput prompt, bool retry)> send_prompt;
  std::function<void()> exit_ui;
  std::function<void(std::string draft)> set_draft;
};

bool execute_slash(SlashHost& host, const std::string& cmd, const std::string& arg,
                   bool extension_request);

} // namespace niminal::app
