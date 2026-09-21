#include "theme.hpp"
#include "transcript.hpp"

#include <ftxui/dom/node.hpp>
#include <ftxui/screen/screen.hpp>

#include <iostream>
#include <string>

using niminal::app::Block;
using niminal::app::BlockKind;
using niminal::app::is_drag_gesture;
using niminal::app::render_transcript_card;
using niminal::app::resolve_theme;
using niminal::app::ThemeMode;
using niminal::app::tool_summary;

static int fail(const char* msg, const std::string& got) {
  std::cerr << msg << "\n got:\n" << got << '\n';
  return 1;
}

static ftxui::Screen render_card_screen(const Block& block) {
  ftxui::Box box;
  auto theme = resolve_theme(ThemeMode::dark);
  auto element = render_transcript_card(block, theme, box);
  ftxui::Screen screen(120, 40);
  ftxui::Render(screen, element.get());
  return screen;
}

static std::string render_card(const Block& block) {
  return render_card_screen(block).ToString();
}

int main() {
  Block thinking{BlockKind::thinking, "line one\nline two"};
  thinking.expanded = false;
  auto compact_thinking = render_card(thinking);
  if (compact_thinking.find("+ Thought") == std::string::npos ||
      compact_thinking.find("line one") != std::string::npos) {
    return fail("compact thinking hides body", compact_thinking);
  }

  thinking.expanded = true;
  auto expanded_thinking = render_card(thinking);
  if (expanded_thinking.find("- Thought") == std::string::npos ||
      expanded_thinking.find("line one") == std::string::npos ||
      expanded_thinking.find("line two") == std::string::npos) {
    return fail("expanded thinking shows body", expanded_thinking);
  }

  Block tool{BlockKind::tool, R"({"command":"ls -la"})"};
  tool.tool_name = "bash";
  tool.result = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9";
  tool.expanded = false;
  auto compact_tool = render_card(tool);
  if (compact_tool.find("$ ls -la") == std::string::npos ||
      compact_tool.find("line1") == std::string::npos ||
      compact_tool.find("line8") == std::string::npos ||
      compact_tool.find("line9") != std::string::npos ||
      compact_tool.find("click to expand") == std::string::npos) {
    return fail("compact bash preview", compact_tool);
  }

  tool.expanded = true;
  auto expanded_tool = render_card(tool);
  if (expanded_tool.find("$ ls -la") == std::string::npos ||
      expanded_tool.find("arguments") != std::string::npos ||
      expanded_tool.find("line9") == std::string::npos) {
    return fail("expanded bash", expanded_tool);
  }

  Block read{BlockKind::tool, R"({"path":"README.md"})"};
  read.tool_name = "read";
  read.expanded = false;
  auto compact_read = render_card(read);
  if (compact_read.find("→ read  README.md") == std::string::npos) {
    return fail("compact read tool", compact_read);
  }

  Block diff{BlockKind::diff, "+added\n-removed", "src/main.cpp", false, "edit"};
  diff.expanded = false;
  auto compact_diff = render_card(diff);
  if (compact_diff.find("→ ✓ edit  src/main.cpp") == std::string::npos ||
      compact_diff.find("+added") != std::string::npos) {
    return fail("compact diff", compact_diff);
  }

  diff.expanded = true;
  auto expanded_diff = render_card(diff);
  if (expanded_diff.find("→ ✓ edit  src/main.cpp") == std::string::npos ||
      expanded_diff.find("+added") == std::string::npos ||
      expanded_diff.find("-removed") == std::string::npos) {
    return fail("expanded diff", expanded_diff);
  }

  // Regression: thinking headers must not share the tool color.
  {
    const auto theme = resolve_theme(ThemeMode::dark);
    if (theme.thinking == theme.meta) {
      return fail("thinking color differs from tool meta color", "identical");
    }
    const auto header_color = [&](const Block& block) {
      const auto screen = render_card_screen(block);
      return screen.PixelAt(0, 0).foreground_color;
    };
    if (header_color(thinking) != theme.thinking) {
      return fail("thinking header uses the thinking color", "wrong color");
    }
    if (header_color(tool) != theme.meta) {
      return fail("tool header uses the meta color", "wrong color");
    }
  }

  if (tool_summary("bash", R"({"command":"pwd"})") != "$ pwd") {
    return fail("bash tool summary", tool_summary("bash", R"({"command":"pwd"})"));
  }
  if (tool_summary("read", R"({"path":"README.md"})") != "→ read  README.md") {
    return fail("read tool summary", tool_summary("read", R"({"path":"README.md"})"));
  }

  // Regression: a click on a card must open the card, not copy a character.
  // FTXUI hands back a one-character selection for a press released on the
  // same cell, so only a pointer that moved counts as a selection drag.
  if (is_drag_gesture(4, 7, 4, 7)) {
    return fail("same-cell press/release is a click", "reported as drag");
  }
  if (!is_drag_gesture(4, 7, 5, 7) || !is_drag_gesture(4, 7, 4, 8)) {
    return fail("moved pointer is a drag", "reported as click");
  }

  return 0;
}
