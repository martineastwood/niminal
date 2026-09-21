#pragma once

#include "theme.hpp"

#include <ftxui/dom/elements.hpp>

#include <string>
#include <string_view>

namespace niminal::app {

ftxui::Element render_markdown(std::string_view text, const Theme& theme);
std::string markdown_outline(std::string_view text);

} // namespace niminal::app
