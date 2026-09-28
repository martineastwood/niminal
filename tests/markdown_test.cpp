#include "markdown.hpp"

#include <ftxui/dom/node.hpp>
#include <ftxui/dom/selection.hpp>
#include <ftxui/screen/screen.hpp>

#include <iostream>
#include <string>
#include <vector>

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

  {
    auto theme = niminal::app::resolve_theme(niminal::app::ThemeMode::dark);
    theme.code = ftxui::Color::RGB(1, 2, 3);
    theme.emphasis = ftxui::Color::RGB(4, 5, 6);
    theme.quote = ftxui::Color::RGB(7, 8, 9);
    theme.muted = ftxui::Color::RGB(10, 11, 12);
    struct HighlightCase {
      std::string source;
      std::string line;
      std::string token;
      int row;
      ftxui::Color expected;
    };
    const std::vector<HighlightCase> cases = {
        {"cpp", "int value = \"text\"; // note", "int", 1, theme.emphasis},
        {"cpp", "int value = \"text\"; // note", "text", 1, theme.quote},
        {"cpp", "int value = \"text\"; // note", "note", 1, theme.muted},
        {"cpp", "\"return\" // if", "return", 1, theme.quote},
        {"cpp", "\"return\" // if", "if", 1, theme.muted},
        {"cpp", "/* open\nstill comment */ int x;", "still comment", 2, theme.muted},
        {"cpp", "/* open\nstill comment */ int x;", "int", 2, theme.emphasis},
        {"json", "{\"ok\": true, \"nil\": null}", "true", 1, theme.emphasis},
        {"json", "{\"ok\": true, \"nil\": null}", "ok", 1, theme.quote},
        {"json", "{\"value\": \"true\"}", "true", 1, theme.quote},
        {"bash", "if [ -n \"$x\" ]; then # note", "if", 1, theme.emphasis},
        {"bash", "if [ -n \"$x\" ]; then # note", "note", 1, theme.muted},
        {"bash", "echo foo#bar # note", "foo#bar", 1, theme.code},
        {"python", "def f(): # note", "def", 1, theme.emphasis},
        {"python", "\"\"\"open\nstill string\"\"\" # note", "\"\"\"", 1, theme.quote},
        {"python", "\"\"\"open\nstill string\"\"\" # note", "\"\"\"", 2, theme.quote},
        {"python", "\"\"\"open\nstill string\"\"\" # note", "still string", 2, theme.quote},
        {"python", "value = \"\"\"open\nstill string\"\"\"", "open", 1, theme.quote},
        {"yaml", "enabled: true # note", "true", 1, theme.emphasis},
        {"yaml", "enabled: true # note", "note", 1, theme.muted},
        {"yaml", "url: https://example.test/#part", "#part", 1, theme.code},
        {"js", "const url = \"https://example.test\"; // note", "const", 1, theme.emphasis},
        {"javascript", "const url = \"https://example.test\"; // note", "https", 1, theme.quote},
        {"jsx", "/* open\nstill comment */ const x = true;", "still comment", 2, theme.muted},
        {"JS", "const x = `hello\nworld`;", "world", 2, theme.quote},
        {"js", "const x = `hello`; return x;", "return", 1, theme.emphasis},
        {"js", "const url = \"https://example.test\"; // note", "note", 1, theme.muted},
        {"ts", "interface User { name: string }", "interface", 1, theme.emphasis},
        {"typescript", "type Count = number;", "number", 1, theme.emphasis},
        {"tsx", "const label = `hello\nworld`;", "world", 2, theme.quote},
        {"ts", "/* open\nstill comment */ type X = string;", "type", 2, theme.emphasis},
        {"rust", "pub fn main() { let message = \"hello\"; }", "fn", 1, theme.emphasis},
        {"rs", "pub fn main() { let message = \"hello\"; }", "hello", 1, theme.quote},
        {"rust", "fn read<'a>(x: &'a str) -> bool { true }", "true", 1, theme.emphasis},
        {"rust", "let x = 1; // note", "note", 1, theme.muted},
        {"rs", "/* open\nstill comment */ let x = true;", "still comment", 2, theme.muted},
        {"go", "func main() { const name = \"hello\" }", "func", 1, theme.emphasis},
        {"golang", "func main() { const name = \"hello\" }", "hello", 1, theme.quote},
        {"go", "const s = `first\nsecond` // note", "second", 2, theme.quote},
        {"go", "const s = `path\\`; return", "return", 1, theme.emphasis},
        {"golang", "/* open\nstill comment */ var x = true", "still comment", 2, theme.muted},
        {"go", "var x = 1 // note", "note", 1, theme.muted},
        {"unknown", "return \"text\" # note", "return", 1, theme.code},
    };
    for (const auto& test : cases) {
      const auto source = "```" + test.source + "\n" + test.line + "\n```";
      auto element = render_markdown(source, theme);
      ftxui::Screen color_screen(80, 5);
      ftxui::Render(color_screen, element);
      const auto code_line = test.row == 1 ? test.line.substr(0, test.line.find('\n'))
                                           : test.line.substr(test.line.find_last_of('\n') + 1);
      const auto x = static_cast<int>(code_line.find(test.token)) + 2;
      if (color_screen.PixelAt(x, test.row).foreground_color != test.expected) {
        return fail("code token color", test.source + "\n" + color_screen.ToString());
      }
    }
    auto element = render_markdown("```json\n{\"ok\": true}\n```", theme);
    ftxui::Screen selection_screen(30, 3);
    ftxui::Selection code_selection(2, 1, 13, 1);
    ftxui::Render(selection_screen, element.get(), code_selection);
    if (code_selection.GetParts() != "{\"ok\": true}") {
      return fail("highlighted code remains selectable", code_selection.GetParts());
    }
    auto triple = render_markdown("```python\nvalue = \"\"\"open\nstill string\"\"\"\n```", theme);
    ftxui::Screen triple_screen(30, 4);
    ftxui::Render(triple_screen, triple);
    auto visible_line = [](const ftxui::Screen& rendered, int row) {
      std::string glyphs;
      for (int x = 0; x < rendered.dimx(); ++x) {
        glyphs += rendered.PixelAt(x, row).character;
      }
      return glyphs;
    };
    if (visible_line(triple_screen, 1).find("value = \"\"\"open") == std::string::npos ||
        visible_line(triple_screen, 2).find("still string\"\"\"") == std::string::npos) {
      return fail("multiline string text preserved", triple_screen.ToString());
    }
    auto templated = render_markdown("```ts\nconst x = `hello\nworld`;\n```", theme);
    ftxui::Screen template_screen(30, 4);
    ftxui::Render(template_screen, templated);
    if (visible_line(template_screen, 1).find("const x = `hello") == std::string::npos ||
        visible_line(template_screen, 2).find("world`;") == std::string::npos) {
      return fail("template string text preserved", template_screen.ToString());
    }
    auto raw = render_markdown("```go\nconst x = `first\nsecond`\n```", theme);
    ftxui::Screen raw_screen(30, 4);
    ftxui::Render(raw_screen, raw);
    if (visible_line(raw_screen, 1).find("const x = `first") == std::string::npos ||
        visible_line(raw_screen, 2).find("second`") == std::string::npos) {
      return fail("Go raw string text preserved", raw_screen.ToString());
    }
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

  // Regression: a word wider than the row wraps instead of running off the
  // right edge. A flexbox cell cannot break, so long words need splitting.
  {
    const std::string url = "https://example.com/a/very/long/reference/page/that/keeps/going/on";
    auto wrapped = render_markdown("see " + url + " and `a_long_inline_code_span_value`", {});
    ftxui::Screen wrap_screen(40, 12);
    ftxui::Render(wrap_screen, wrapped);
    std::string glyphs;
    for (int y = 0; y < wrap_screen.dimy(); ++y) {
      for (int x = 0; x < wrap_screen.dimx(); ++x) {
        const auto& character = wrap_screen.PixelAt(x, y).character;
        if (!character.empty() && character != " ") {
          glyphs += character;
        }
      }
    }
    std::string wanted = "see" + url + "anda_long_inline_code_span_value";
    std::erase(wanted, ' ');
    if (glyphs.find(wanted) == std::string::npos) {
      return fail("long markdown words wrap", wrap_screen.ToString());
    }
  }

  return 0;
}
