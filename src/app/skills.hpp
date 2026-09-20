#pragma once

#include <filesystem>
#include <string>
#include <vector>

namespace niminal {
struct Tool;
}

namespace niminal::app {

struct Skill {
  std::string name;
  std::string description;
  std::filesystem::path path;
};

std::vector<Skill> discover_skills(const std::filesystem::path& workspace);
std::string load_skill(const std::filesystem::path& workspace,
                       const std::string& name);
niminal::Tool skill_tool(const std::filesystem::path& workspace);

}  // namespace niminal::app
