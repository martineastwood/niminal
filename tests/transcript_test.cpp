#include "theme.hpp"
#include "transcript.hpp"

#include <ftxui/dom/node.hpp>
#include <ftxui/screen/screen.hpp>

#include <iostream>
#include <string>

using niminal::app::Block;
using niminal::app::BlockKind;
using niminal::app::render_transcript_card;
using niminal::app::resolve_theme;
using niminal::app::ThemeMode;
using niminal::app::tool_summary;

static int fail(const char* msg, const std::string& got) {
  std::cerr << msg << "\n got:\n" << got << '\n';
  return 1;
}

static std::string render_card(const Block& block) {
  ftxui::Box box;
  auto theme = resolve_theme(ThemeMode::dark);
  auto element = render_transcript_card(block, theme, box);
  ftxui::Screen screen(120, 40);
  ftxui::Render(screen, element.get());
  return screen.ToString();
}

int main() {
  Block thinking{BlockKind::thinking, "line one\nline two"};
  thinking.expanded = false;
  auto compact_thinking = render_card(thinking);
  if (compact_thinking.find("▸ thinking") == std::string::npos ||
      compact_thinking.find("line one") != std::string::npos) {
    return fail("compact thinking hides body", compact_thinking);
  }

  thinking.expanded = true;
  auto expanded_thinking = render_card(thinking);
  if (expanded_thinking.find("▾ thinking") == std::string::npos ||
      expanded_thinking.find("line one") == std::string::npos ||
      expanded_thinking.find("line two") == std::string::npos) {
    return fail("expanded thinking shows body", expanded_thinking);
  }

  Block tool{BlockKind::tool, R"({"command":"ls -la"})"};
  tool.tool_name = "bash";
  tool.result = "total 8";
  tool.expanded = false;
  auto compact_tool = render_card(tool);
  if (compact_tool.find("▸ bash") == std::string::npos ||
      compact_tool.find("$ ls -la") == std::string::npos ||
      compact_tool.find("total 8") != std::string::npos) {
    return fail("compact tool", compact_tool);
  }

  tool.expanded = true;
  auto expanded_tool = render_card(tool);
  if (expanded_tool.find("▾ bash") == std::string::npos ||
      expanded_tool.find("arguments") == std::string::npos ||
      expanded_tool.find("ls -la") == std::string::npos ||
      expanded_tool.find("result") == std::string::npos ||
      expanded_tool.find("total 8") == std::string::npos) {
    return fail("expanded tool", expanded_tool);
  }

  Block diff{BlockKind::diff, "+added\n-removed", "src/main.cpp", false, "edit"};
  diff.expanded = false;
  auto compact_diff = render_card(diff);
  if (compact_diff.find("▸ ✓ edit") == std::string::npos ||
      compact_diff.find("src/main.cpp") == std::string::npos ||
      compact_diff.find("+added") != std::string::npos) {
    return fail("compact diff", compact_diff);
  }

  diff.expanded = true;
  auto expanded_diff = render_card(diff);
  if (expanded_diff.find("▾") == std::string::npos ||
      expanded_diff.find("src/main.cpp") == std::string::npos ||
      expanded_diff.find("+added") == std::string::npos ||
      expanded_diff.find("-removed") == std::string::npos) {
    return fail("expanded diff", expanded_diff);
  }

  if (tool_summary("bash", R"({"command":"pwd"})") != "▸ bash  $ pwd") {
    return fail("tool summary", tool_summary("bash", R"({"command":"pwd"})"));
  }

  return 0;
}
