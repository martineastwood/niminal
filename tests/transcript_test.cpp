#include "theme.hpp"
#include "transcript.hpp"

#include <ftxui/dom/node.hpp>
#include <ftxui/screen/screen.hpp>
#include <nlohmann/json.hpp>

#include <iostream>
#include <string>

using niminal::app::Block;
using niminal::app::BlockKind;
using niminal::app::blocks_from_events;
using niminal::app::is_drag_gesture;
using niminal::app::render_transcript_card;
using niminal::app::render_user_message;
using niminal::app::resolve_theme;
using niminal::app::ThemeMode;

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

  Block long_bash{BlockKind::tool, R"({"command":"yes"})"};
  long_bash.tool_name = "bash";
  long_bash.expanded = true;
  for (int i = 1; i <= 150; ++i) {
    long_bash.result += "row-" + std::to_string(i) + "\n";
  }
  ftxui::Box long_box;
  auto long_element = render_transcript_card(long_bash, resolve_theme(ThemeMode::dark), long_box);
  ftxui::Screen long_screen(120, 200);
  ftxui::Render(long_screen, long_element.get());
  auto long_rendered = long_screen.ToString();
  if (long_rendered.find("row-1") == std::string::npos ||
      long_rendered.find("row-150") == std::string::npos) {
    return fail("expanded bash keeps full output", long_rendered);
  }

  // Regression: a long transcript must expose the viewport scrollbar.
  {
    ftxui::Elements rows;
    for (int i = 0; i < 20; ++i) {
      rows.push_back(ftxui::text("row-" + std::to_string(i)));
    }
    auto transcript = ftxui::vbox(std::move(rows));
    auto element = transcript | ftxui::focusPositionRelative(0.F, 1.F) |
                   ftxui::vscroll_indicator | ftxui::yframe | ftxui::yflex;
    ftxui::Screen screen(30, 8);
    ftxui::Render(screen, element);
    if (transcript->requirement().min_y != 20) {
      return fail("rendered transcript reports its scroll height", screen.ToString());
    }
    const auto rendered = screen.ToString();
    if (rendered.find("┃") == std::string::npos && rendered.find("╻") == std::string::npos &&
        rendered.find("╹") == std::string::npos) {
      return fail("long transcript shows scrollbar", rendered);
    }
  }

  {
    Block user{BlockKind::user, "A reusable user message"};
    auto element = render_user_message(user, resolve_theme(ThemeMode::dark));
    ftxui::Screen first(40, 4);
    ftxui::Render(first, element);
    ftxui::Screen second(30, 4);
    ftxui::Render(second, element);
    if (first.ToString().find(user.text) == std::string::npos ||
        second.ToString().find(user.text) == std::string::npos) {
      return fail("cached user message renders at different widths", second.ToString());
    }
  }

  Block read{BlockKind::tool, R"({"path":"README.md"})"};
  read.tool_name = "read";
  read.expanded = false;
  auto compact_read = render_card(read);
  if (compact_read.find("→ read  README.md") == std::string::npos) {
    return fail("compact read tool", compact_read);
  }

  Block diff{BlockKind::diff, "+added\n-removed", "src/main.cpp", "edit"};
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

  // Regression: a click on a card must open the card, not copy a character.
  // FTXUI hands back a one-character selection for a press released on the
  // same cell, so only a pointer that moved counts as a selection drag.
  if (is_drag_gesture(4, 7, 4, 7)) {
    return fail("same-cell press/release is a click", "reported as drag");
  }
  if (!is_drag_gesture(4, 7, 5, 7) || !is_drag_gesture(4, 7, 4, 8)) {
    return fail("moved pointer is a drag", "reported as click");
  }

  using json = nlohmann::json;
  const std::vector<json> events = {
      json{{"type", "user"}, {"content", json::array({{{"type", "text"}, {"text", "hello"}}})}},
      json{{"type", "assistant"},
           {"content", json::array({{{"type", "text"}, {"text", "hi"}},
                                    {{"type", "tool_use"},
                                     {"id", "t1"},
                                     {"name", "read"},
                                     {"input", json{{"path", "README.md"}}}}})}},
      json{{"type", "tool_result"}, {"id", "t1"}, {"output", "done"}}};
  auto blocks = blocks_from_events(events);
  if (blocks.size() != 3 || blocks[0].kind != BlockKind::user || blocks[0].text != "hello" ||
      blocks[1].kind != BlockKind::assistant || blocks[1].text != "hi" ||
      blocks[2].kind != BlockKind::tool || blocks[2].tool_name != "read" ||
      blocks[2].result != "done") {
    return fail("blocks_from_events projects session events", std::to_string(blocks.size()));
  }

  return 0;
}
