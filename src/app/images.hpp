#pragma once

#include <niminal/types.hpp>

#include <filesystem>
#include <string>
#include <string_view>

namespace niminal::app {

bool image_path(std::string_view path);
niminal::json image_part(std::string_view bytes, std::string_view name);
niminal::json clipboard_image_part(std::string_view bytes);
niminal::json read_image(const std::filesystem::path& path);

} // namespace niminal::app
