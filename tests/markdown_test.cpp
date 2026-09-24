#include "markdown.hpp"

#include <ftxui/dom/node.hpp>
#include <ftxui/dom/selection.hpp>
#include <ftxui/screen/screen.hpp>

#include <iostream>
#include <string>

using niminal::app::markdown_outline;
using niminal::app::render_markdown;

static int fail(const char* msg, const std::string& got) {
  std::cerr << msg << "\n got:\n" << got << '\n';
  return 1;
}

int main() {
  auto heading = markdown_outline("# Hello **world**");
  if (heading.find("H1 Hello [b]world[/b]") == std::string::npos) {
    return fail("heading+bold", heading);
  }

  auto list = markdown_outline("- one\n- two `x`");
  if (list.find("LI • one") == std::string::npos ||
      list.find("LI • two [c]x[/c]") == std::string::npos) {
    return fail("list", list);
  }

  auto fence = markdown_outline("```cpp\nint x;\n```");
  if (fence.find("CODE cpp") == std::string::npos || fence.find("  int x;") == std::string::npos) {
    return fail("fence", fence);
  }

  auto open = markdown_outline("```\nstill streaming");
  if (open.find("CODE") == std::string::npos ||
      open.find("  still streaming") == std::string::npos) {
    return fail("unclosed fence", open);
  }

  auto link = markdown_outline("[docs](https://ex)");
  if (link.find("[u]docs[/u][d] (https://ex)[/d]") == std::string::npos) {
    return fail("link", link);
  }

  auto table = markdown_outline("| A | B |\n| --- | --- |\n| 1 | 2 |");
  if (table.find("TABLE 2") == std::string::npos) {
    return fail("table", table);
  }

  auto quote = markdown_outline("> note");
  if (quote.find("Q note") == std::string::npos) {
    return fail("quote", quote);
  }

  auto nested = markdown_outline("```markdown\n# Header 1\n- Item 1\n```");
  if (nested.find("CODE") != std::string::npos) {
    return fail("md fence should render", nested);
  }
  if (nested.find("H1 Header 1") == std::string::npos ||
      nested.find("LI • Item 1") == std::string::npos) {
    return fail("md fence nested", nested);
  }

  auto cpp = markdown_outline("```cpp\n# not a heading\n```");
  if (cpp.find("CODE cpp") == std::string::npos ||
      cpp.find("  # not a heading") == std::string::npos) {
    return fail("cpp fence stays code", cpp);
  }

  auto under = markdown_outline("_italic_ and __bold__ and a_b_c");
  if (under.find("[i]italic[/i]") == std::string::npos ||
      under.find("[b]bold[/b]") == std::string::npos || under.find("a_b_c") == std::string::npos) {
    return fail("underscore emphasis", under);
  }

  auto rendered = render_markdown("first\n\nsecond", {});
  ftxui::Screen screen(20, 3);
  ftxui::Selection selection(0, 0, 19, 2);
  ftxui::Render(screen, rendered.get(), selection);
  if (selection.GetParts() != "first\n\nsecond") {
    return fail("markdown selection should preserve blank lines", selection.GetParts());
  }

  auto streaming =
      render_markdown("**Answer** A sentence with several words that keeps growing", {});
  ftxui::Screen streaming_screen(80, 2);
  ftxui::Selection streaming_selection(0, 0, 79, 0);
  ftxui::Render(streaming_screen, streaming.get(), streaming_selection);
  if (streaming_selection.GetParts().find(
          "Answer A sentence with several words that keeps growing") == std::string::npos) {
    return fail("styled streaming paragraph keeps every word", streaming_selection.GetParts());
  }

  return 0;
}
