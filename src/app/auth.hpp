#pragma once

#include <filesystem>
#include <string>
#include <string_view>

namespace niminal::app {

std::filesystem::path auth_path();
std::string read_auth_key(std::string_view provider);
std::string read_auth_key(std::string_view provider, const std::filesystem::path& path);

} // namespace niminal::app
