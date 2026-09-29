#include "widget_panel.hpp"

#include <algorithm>
#include <utility>

namespace niminal::app {

using namespace ftxui;

int widget_panel_max_offset(int body_rows, int height) {
  return std::max(0, body_rows - height);
}

void scroll_widget_panel(WidgetPanelScroll& scroll, int body_rows, int height, int delta) {
  const int bottom = widget_panel_max_offset(body_rows, height);
  scroll.offset = std::clamp(scroll.offset + delta, 0, bottom);
  scroll.stick = bottom > 0 && scroll.offset == bottom;
}

int widget_panel_offset(WidgetPanelScroll& scroll, int height) {
  const int bottom = widget_panel_max_offset(scroll.rows, height);
  scroll.offset = scroll.stick ? bottom : std::clamp(scroll.offset, 0, bottom);
  return scroll.offset;
}

// A yframe scrolls to keep the focused row in the middle of the window, so ask
// for the row that puts the first visible one at the top edge. Without that
// offset the first arrow presses only move the focus inside the window.
ftxui::Element widget_panel_window(ftxui::Element body, int offset, int height) {
  return std::move(body) | focusPosition(0, offset + (height - 1) / 2) | vscroll_indicator |
         yframe | size(HEIGHT, EQUAL, height);
}

} // namespace niminal::app
