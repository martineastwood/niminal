#pragma once

#include <niminal/types.hpp>
#include <optional>
#include <string>

namespace niminal::app {

void copy_to_clipboard(const std::string& text);
std::string paste_from_clipboard();
std::optional<niminal::json> paste_image_from_clipboard();

} // namespace niminal::app
