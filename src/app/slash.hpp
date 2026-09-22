#pragma once

#include <filesystem>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace niminal::app {

struct ExtensionCommand;
class ExtensionRuntime;
class Session;
class Keybindings;

struct Suggestion {
  std::string fill;
  std::string label;
  bool file = false;
};

struct UserBashRequest {
  std::string command;
  bool exclude_from_context = false;
};

std::optional<UserBashRequest> parse_user_bash(std::string_view prompt);
std::pair<std::string, std::string> split_slash(const std::string& prompt);
bool is_builtin_slash(std::string_view command);
std::string slash_help(const Keybindings& keybindings);
std::vector<Suggestion>
slash_suggestions(const std::string& draft, const std::filesystem::path& dir,
                  const std::string& workspace, std::string_view provider, std::string_view model,
                  const std::vector<std::string>& recents,
                  const std::vector<ExtensionCommand>& extension_commands, const Session& session);

std::optional<std::string> skill_slash_error(const std::filesystem::path& cwd,
                                             std::string_view cmd);
std::optional<std::string> resolve_prompt_template(const std::filesystem::path& cwd,
                                                   const std::string& prompt, std::string_view cmd,
                                                   std::string_view arg);
bool is_extension_slash(const std::shared_ptr<ExtensionRuntime>& extensions, std::string_view cmd);

} // namespace niminal::app
