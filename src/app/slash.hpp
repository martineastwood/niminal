#pragma once

#include <filesystem>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace niminal::app {

struct ExtensionCommand;
class Session;

struct Suggestion {
  std::string fill;
  std::string label;
  bool file = false;
};

std::string trim_copy(std::string s);
std::pair<std::string, std::string> split_slash(const std::string& prompt);
bool is_builtin_slash(std::string_view command);
std::string slash_help();
std::vector<Suggestion> slash_suggestions(const std::string& draft,
                                          const std::filesystem::path& dir,
                                          const std::string& workspace, std::string_view provider,
                                          std::string_view model,
                                          const std::vector<std::string>& recents,
                                          const std::vector<ExtensionCommand>& extension_commands,
                                          const Session& session);

} // namespace niminal::app
