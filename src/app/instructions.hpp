#pragma once

#include <filesystem>
#include <string>
#include <vector>

namespace niminal::app {

std::filesystem::path global_agents_path();
std::vector<std::filesystem::path> instruction_paths(
    const std::filesystem::path& workspace);
std::string load_project_instructions(const std::filesystem::path& workspace);
std::string load_scoped_instructions(const std::filesystem::path& workspace,
                                     const std::filesystem::path& target);

}  // namespace niminal::app
