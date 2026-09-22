#pragma once

#include <filesystem>
#include <string>
#include <vector>

namespace niminal::app {

void set_context_files_enabled(bool enabled);

std::string load_system_prompt(const std::filesystem::path& workspace);
std::string load_append_system_prompt(const std::filesystem::path& workspace);
std::string load_project_instructions(const std::filesystem::path& workspace);
std::string load_scoped_instructions(const std::filesystem::path& workspace,
                                     const std::filesystem::path& target);

} // namespace niminal::app
