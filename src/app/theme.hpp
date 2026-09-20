#pragma once

#include <ftxui/dom/elements.hpp>

#include <optional>
#include <string_view>

namespace niminal::app {

enum class ThemeMode { automatic, light, dark };

struct Theme {
  ftxui::Color accent;    // user turns, badges, activity, headings
  ftxui::Color code;      // inline code and fenced blocks
  ftxui::Color add;       // added diff lines
  ftxui::Color del;       // removed diff lines
  ftxui::Color meta;      // tool calls, approvals, diff markers
  ftxui::Color error;
  ftxui::Color muted;     // status lines, diff context
  ftxui::Color emphasis;  // bold spans
  ftxui::Color italic;
  ftxui::Color quote;
  ftxui::Color input_fg;
  ftxui::Color input_bg;
  ftxui::Color hover_bg;
};

std::optional<ThemeMode> parse_theme_mode(std::string_view value);
const char* theme_mode_name(ThemeMode mode);

// Parses the reply to an OSC 11 query, such as "rgb:0000/0000/0000".
std::optional<ThemeMode> theme_from_osc_reply(std::string_view reply);
// Parses COLORFGBG; its last field is the background color index.
std::optional<ThemeMode> theme_from_colorfgbg(std::string_view value);

// Asks the terminal for its background once, falls back to COLORFGBG, and
// assumes dark when neither answers.
ThemeMode detect_terminal_theme();

Theme resolve_theme(ThemeMode mode);

}  // namespace niminal::app
