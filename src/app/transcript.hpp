#pragma once

#include "theme.hpp"

#include <ftxui/dom/elements.hpp>

#include <cstddef>
#include <string>
#include <string_view>
#include <utility>

namespace niminal::app {

enum class BlockKind {
  user,
  assistant,
  thinking,
  tool,
  diff,
  error,
  status,
  approval,
};

struct Block {
  BlockKind kind = BlockKind::assistant;
  std::string text;
  std::string path;
  bool created = false;
  std::string tool_name;
  std::string tool_id;

  Block() = default;
  Block(BlockKind kind, std::string text) : kind(kind), text(std::move(text)) {}
  Block(BlockKind kind, std::string text, std::string path, bool created)
      : kind(kind), text(std::move(text)), path(std::move(path)), created(created) {}
  Block(BlockKind kind, std::string text, std::string path, bool created, std::string tool_name)
      : kind(kind), text(std::move(text)), path(std::move(path)), created(created),
        tool_name(std::move(tool_name)) {}
};

ftxui::Element render_diff_card(const Block& block, const Theme& theme);
std::string clip_text(std::string text, size_t max_chars, int max_lines);
std::string tool_summary(const std::string& name, const std::string& args);
ftxui::Decorator block_style(BlockKind kind, const Theme& theme);
const char* block_label(BlockKind kind);
ftxui::Element paragraph_preserving_whitespace(std::string_view value);

} // namespace niminal::app
