#pragma once

#include <string>

namespace niminal::app {

void copy_to_clipboard(const std::string& text);
std::string paste_from_clipboard();

} // namespace niminal::app
