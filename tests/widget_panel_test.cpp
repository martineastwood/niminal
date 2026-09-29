#include "markdown.hpp"
#include "theme.hpp"
#include "widget_panel.hpp"

#include <ftxui/dom/node.hpp>
#include <ftxui/screen/screen.hpp>

#include <iostream>
#include <string>
#include <vector>

using niminal::app::render_markdown;
using niminal::app::resolve_theme;
using niminal::app::scroll_widget_panel;
using niminal::app::ThemeMode;
using niminal::app::widget_panel_offset;
using niminal::app::widget_panel_window;
using niminal::app::WidgetPanelScroll;

namespace {

int failures = 0;

void check(bool condition, const std::string& message) {
  if (!condition) {
    std::cerr << "widget_panel: " << message << '\n';
    ++failures;
  }
}

// A panel the way the TUI drives it: each frame draws the window at the current
// offset and measures the body for the next one, because a markdown body only
// reports the height it wraps to after a frame has laid it out.
struct Panel {
  ftxui::Element body;
  WidgetPanelScroll scroll;
  int height = 0;
  int width = 0;

  std::vector<std::string> draw() {
    auto window = widget_panel_window(body, widget_panel_offset(scroll, height), height);
    ftxui::Screen screen(width, height);
    ftxui::Render(screen, window.get());
    scroll.rows = body->requirement().min_y;
    std::vector<std::string> rows;
    for (int y = 0; y < screen.dimy(); ++y) {
      std::string row;
      for (int x = 0; x < 20; ++x) {
        row += screen.PixelAt(x, y).character;
      }
      rows.push_back(std::move(row));
    }
    return rows;
  }
};

// One list item per row and no trailing blank line, so the body is exactly
// |rows| rows tall once it is laid out.
std::string bullet_body(int rows) {
  std::string text;
  for (int i = 1; i <= rows; ++i) {
    if (i > 1) {
      text += '\n';
    }
    text += "- row " + std::to_string(i);
  }
  return text;
}

ftxui::Element body_of(int rows) {
  return render_markdown(bullet_body(rows), resolve_theme(ThemeMode::dark));
}

Panel make_panel(int rows, int height) {
  Panel panel{body_of(rows), {}, height, 60};
  panel.draw(); // the first frame is where the body learns the panel's width
  return panel;
}

// Row index of the first body row on screen, read back from its label.
int first_row(const std::vector<std::string>& screen_rows) {
  for (const auto& row : screen_rows) {
    const auto at = row.find("row ");
    if (at != std::string::npos) {
      return std::stoi(row.substr(at + 4)) - 1;
    }
  }
  return -1;
}

} // namespace

int main() {
  // A body one row taller than the window still scrolls: the first arrow press
  // used to move the focus inside the window before anything moved on screen.
  {
    auto panel = make_panel(13, 12);
    check(panel.scroll.rows == 13,
          "expected a 13 row body, got " + std::to_string(panel.scroll.rows));
    check(first_row(panel.draw()) == 0, "a fresh panel starts at the first row");
    scroll_widget_panel(panel.scroll, 13, 12, 1);
    check(panel.scroll.offset == 1, "one press scrolls one row");
    check(first_row(panel.draw()) == 1, "the window follows the offset");
  }

  // The window scrolls one row per press all the way to the end, then stops.
  {
    auto panel = make_panel(20, 12);
    for (int press = 0; press < 8; ++press) {
      scroll_widget_panel(panel.scroll, 20, 12, 1);
      check(first_row(panel.draw()) == press + 1,
            "press " + std::to_string(press + 1) + " shows row " + std::to_string(press + 2));
    }
    check(panel.scroll.offset == 8 && panel.scroll.stick, "the last row sticks to the tail");
    scroll_widget_panel(panel.scroll, 20, 12, 1);
    check(panel.scroll.offset == 8, "scrolling past the end does nothing");
    scroll_widget_panel(panel.scroll, 20, 12, -1);
    check(panel.scroll.offset == 7 && !panel.scroll.stick, "scrolling back releases the tail");
    scroll_widget_panel(panel.scroll, 20, 12, -99);
    check(panel.scroll.offset == 0, "scrolling past the top does nothing");
  }

  // A page key moves one window, and a body that fits the window never moves.
  {
    auto panel = make_panel(40, 12);
    scroll_widget_panel(panel.scroll, 40, 12, 12);
    check(first_row(panel.draw()) == 12, "page down shows row 13");
    scroll_widget_panel(panel.scroll, 40, 12, -12);
    check(first_row(panel.draw()) == 0, "page up returns to the first row");
  }
  {
    auto panel = make_panel(6, 12);
    scroll_widget_panel(panel.scroll, 6, 12, 1);
    check(panel.scroll.offset == 0 && !panel.scroll.stick, "a body that fits does not scroll");
  }

  // A body that shrinks under the current offset is pulled back into range.
  {
    auto panel = make_panel(30, 12);
    scroll_widget_panel(panel.scroll, 30, 12, 18);
    check(panel.scroll.offset == 18, "scrolled to the end of a long body");
    panel.body = body_of(14);
    panel.draw(); // measures the new body
    panel.draw(); // the frame after it clamps the offset to the shorter body
    check(panel.scroll.offset == 2, "the offset is clamped to the shorter body");
    check(first_row(panel.draw()) == 2, "a shortened body keeps its last rows visible");
  }

  if (failures != 0) {
    std::cerr << failures << " widget panel check(s) failed\n";
    return 1;
  }
  return 0;
}
