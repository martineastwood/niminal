#pragma once

#include <ftxui/dom/elements.hpp>

#include <string>
#include <string_view>

namespace niminal::app {

ftxui::Element render_markdown(std::string_view text);
std::string markdown_outline(std::string_view text);

}  // namespace niminal::app
