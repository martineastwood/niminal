#pragma once

#include <filesystem>
#include <string>
#include <vector>

namespace niminal::app {

std::string load_project_instructions(const std::filesystem::path& workspace);
std::string load_scoped_instructions(const std::filesystem::path& workspace,
                                     const std::filesystem::path& target);

} // namespace niminal::app
