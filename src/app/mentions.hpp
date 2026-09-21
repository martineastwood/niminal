#pragma once

#include "workspace.hpp"

#include <cstddef>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace niminal::app {

struct FileMention {
  size_t start;
  size_t end;
  std::string query;
};

std::optional<FileMention> file_mention_at(std::string_view text, size_t cursor);
std::vector<std::string> suggest_mentioned_files(const Workspace& workspace, std::string_view query,
                                                 size_t limit = 8);
std::string apply_file_mention(std::string_view text, size_t cursor, std::string_view path);
std::string expand_file_mentions(const Workspace& workspace, std::string_view prompt);

} // namespace niminal::app
