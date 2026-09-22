#pragma once

#include "theme.hpp"

#include <ftxui/dom/elements.hpp>
#include <ftxui/screen/box.hpp>
#include <nlohmann/json_fwd.hpp>

#include <cstddef>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

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
  std::string result;
  std::string path;
  bool created = false;
  bool expanded = false;
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

bool is_card_block(BlockKind kind);
// FTXUI reports a one-character selection for a press released on the same
// cell, so only a pointer that moved between press and release is a selection
// gesture. Anything else is a click.
bool is_drag_gesture(int press_x, int press_y, int release_x, int release_y);
ftxui::Element render_diff_card(const Block& block, const Theme& theme);
ftxui::Element render_user_message(const Block& block, const Theme& theme);
ftxui::Element render_transcript_card(const Block& block, const Theme& theme, ftxui::Box& box);
std::vector<Block> blocks_from_events(const std::vector<nlohmann::json>& events);
std::string clip_text(std::string text, size_t max_chars, int max_lines);
std::string tool_summary(const std::string& name, const std::string& args);
ftxui::Decorator block_style(BlockKind kind, const Theme& theme);
const char* block_label(BlockKind kind);
ftxui::Element paragraph_preserving_whitespace(std::string_view value);

} // namespace niminal::app
