#pragma once

#include "theme.hpp"

#include <niminal/json.hpp>

#include <ftxui/dom/elements.hpp>
#include <ftxui/screen/box.hpp>

#include <cstddef>
#include <filesystem>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace niminal::app {

class ExtensionRuntime;

// Names the intro screen lists. Read from the live workspace and extension
// runtime on every call, so a reload shows up without restarting Niminal.
struct WelcomeCatalog {
  std::vector<std::string> skills;
  std::vector<std::string> extensions;
  std::vector<std::string> tools;
};

WelcomeCatalog welcome_catalog(const std::filesystem::path& cwd,
                               const ExtensionRuntime* extensions);

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
  bool expanded = false;
  std::string tool_name;
  std::string tool_id;
  std::string turn_id;
  int step = -1;

  Block() = default;
  Block(BlockKind block_kind, std::string block_text)
      : kind(block_kind), text(std::move(block_text)) {}
  Block(BlockKind block_kind, std::string block_text, std::string block_path,
        std::string block_tool_name)
      : kind(block_kind), text(std::move(block_text)), path(std::move(block_path)),
        tool_name(std::move(block_tool_name)) {}
};

bool is_card_block(BlockKind kind);
bool is_conversation_block(BlockKind kind);
// FTXUI reports a one-character selection for a press released on the same
// cell, so only a pointer that moved between press and release is a selection
// gesture. Anything else is a click.
bool is_drag_gesture(int press_x, int press_y, int release_x, int release_y);
int measure_transcript_height(const ftxui::Element& element, int width);
ftxui::Element virtual_transcript(ftxui::Elements entries, const std::vector<int>& heights);
ftxui::Element render_diff_card(const Block& block, const Theme& theme);
ftxui::Element render_approval_block(const Block& block, const Theme& theme);
ftxui::Element render_user_message(const Block& block, const Theme& theme);
ftxui::Element render_welcome_screen(const std::string& title, const std::string& model_line,
                                     const std::vector<std::string>& skills,
                                     const std::vector<std::string>& extensions,
                                     const std::vector<std::string>& tools, int width,
                                     const Theme& theme);
ftxui::Element render_transcript_card(const Block& block, const Theme& theme, ftxui::Box& box);
std::vector<Block> blocks_from_events(const std::vector<niminal::json>& events);
std::string clip_text(std::string text, size_t max_chars, int max_lines);
ftxui::Decorator block_style(BlockKind kind, const Theme& theme);
const char* block_label(BlockKind kind);
ftxui::Element paragraph_preserving_whitespace(std::string_view value);

} // namespace niminal::app
