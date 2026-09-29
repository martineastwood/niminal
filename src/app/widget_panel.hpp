#pragma once

#include <ftxui/dom/elements.hpp>

namespace niminal::app {

// Scroll state for one extension widget panel, keyed by "extension:key" so a
// panel keeps its place across frames. |offset| is the first body row the
// window shows, |rows| the rows the body needed when it was last rendered, and
// |stick| whether the panel follows the tail of a growing body.
struct WidgetPanelScroll {
  int offset = 0;
  int rows = 0;
  bool stick = false;
};

// Rows the panel scrolls before its last row reaches the bottom edge.
int widget_panel_max_offset(int body_rows, int height);

// Scrolls |delta| rows: one for an arrow key, a window for a page. Stops at
// both ends, and follows the tail once the last row is on screen.
void scroll_widget_panel(WidgetPanelScroll& scroll, int body_rows, int height, int delta);

// Clamps |scroll| to the height of the body it last rendered, and returns the
// first visible row.
int widget_panel_offset(WidgetPanelScroll& scroll, int height);

// Shows |body| in a window |height| rows tall, scrolled to |offset|.
ftxui::Element widget_panel_window(ftxui::Element body, int offset, int height);

} // namespace niminal::app
