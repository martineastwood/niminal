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
using niminal::app::measure_transcript_height;
using niminal::app::render_transcript_card;
using niminal::app::render_user_message;
using niminal::app::resolve_theme;
using niminal::app::ThemeMode;
using niminal::app::virtual_transcript;

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
    std::vector<int> heights;
    for (int i = 0; i < 20; ++i) {
      rows.push_back(ftxui::text("row-" + std::to_string(i)));
      heights.push_back(1);
    }
    auto transcript = virtual_transcript(std::move(rows), heights);
    auto element = transcript | ftxui::focusPositionRelative(0.F, 1.F) | ftxui::vscroll_indicator |
                   ftxui::yframe | ftxui::yflex;
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
    if (rendered.find("row-19") == std::string::npos ||
        rendered.find("row-0") != std::string::npos) {
      return fail("virtual transcript draws bottom viewport", rendered);
    }
  }

  {
    ftxui::Elements full_rows;
    ftxui::Elements cached_rows;
    std::vector<int> heights;
    for (int i = 0; i < 20; ++i) {
      const auto row = "row-" + std::to_string(i);
      full_rows.push_back(ftxui::text(row));
      cached_rows.push_back(ftxui::text(row));
      heights.push_back(1);
    }
    auto cached = virtual_transcript(std::move(cached_rows), heights);
    for (float position : {0.F, 0.5F, 1.F}) {
      auto full = ftxui::vbox(full_rows) | ftxui::focusPositionRelative(0.F, position) |
                  ftxui::vscroll_indicator | ftxui::yframe | ftxui::yflex;
      auto visible = cached | ftxui::focusPositionRelative(0.F, position) |
                     ftxui::vscroll_indicator | ftxui::yframe | ftxui::yflex;
      ftxui::Screen full_screen(30, 8);
      ftxui::Screen cached_screen(30, 8);
      ftxui::Render(full_screen, full);
      ftxui::Render(cached_screen, visible);
      if (full_screen.ToString() != cached_screen.ToString()) {
        return fail("cached virtual transcript updates across scroll positions",
                    cached_screen.ToString());
      }
    }
  }

  {
    auto paragraph = ftxui::paragraph("one two three four five six seven eight nine ten");
    const int measured = measure_transcript_height(paragraph, 12);
    auto comparison = ftxui::paragraph("one two three four five six seven eight nine ten");
    ftxui::Screen full(12, 20);
    ftxui::Render(full, comparison);
    if (measured <= 1 || measured != comparison->requirement().min_y) {
      return fail("virtual transcript measures wrapped height", std::to_string(measured));
    }
  }

  for (int width : {40, 65}) {
    for (float position : {0.F, 0.5F, 1.F}) {
      Block user{BlockKind::user,
                 "A long user question with enough words to wrap across several terminal lines"};
      Block thought{BlockKind::thinking, "First thought line\nSecond thought line"};
      thought.expanded = true;
      ftxui::Box full_box;
      ftxui::Box virtual_box;
      auto theme = resolve_theme(ThemeMode::dark);
      ftxui::Elements full_entries{
          ftxui::vbox({render_user_message(user, theme), ftxui::text("")}),
          ftxui::vbox({ftxui::paragraph("Assistant text wraps here and keeps going for a while."),
                       ftxui::text("")}),
          ftxui::vbox({render_transcript_card(thought, theme, full_box), ftxui::text("")})};
      ftxui::Elements visible_entries{
          ftxui::vbox({render_user_message(user, theme), ftxui::text("")}),
          ftxui::vbox({ftxui::paragraph("Assistant text wraps here and keeps going for a while."),
                       ftxui::text("")}),
          ftxui::vbox({render_transcript_card(thought, theme, virtual_box), ftxui::text("")})};
      std::vector<int> heights;
      for (const auto& entry : visible_entries) {
        heights.push_back(measure_transcript_height(entry, width - 1));
      }
      virtual_box = {};
      auto full = ftxui::vbox(std::move(full_entries)) |
                  ftxui::focusPositionRelative(0.F, position) | ftxui::vscroll_indicator |
                  ftxui::yframe | ftxui::yflex;
      auto visible = virtual_transcript(std::move(visible_entries), heights) |
                     ftxui::focusPositionRelative(0.F, position) | ftxui::vscroll_indicator |
                     ftxui::yframe | ftxui::yflex;
      ftxui::Screen full_screen(width, 8);
      ftxui::Screen visible_screen(width, 8);
      ftxui::Render(full_screen, full);
      ftxui::Render(visible_screen, visible);
      if (full_screen.ToString() != visible_screen.ToString()) {
        return fail("virtual transcript matches full rendering", visible_screen.ToString());
      }
      ftxui::Selection full_selection(0, 0, width - 2, 7);
      ftxui::Selection visible_selection(0, 0, width - 2, 7);
      const auto full_text = ftxui::GetNodeSelectedContent(full_screen, full.get(), full_selection);
      const auto visible_text =
          ftxui::GetNodeSelectedContent(visible_screen, visible.get(), visible_selection);
      if (full_text != visible_text) {
        return fail("virtual transcript preserves selection", visible_text);
      }
      if (full_screen.ToString().find("Thought") != std::string::npos && full_box != virtual_box) {
        return fail("virtual transcript preserves card hit box", visible_screen.ToString());
      }
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
