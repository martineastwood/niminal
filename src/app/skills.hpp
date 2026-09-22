#pragma once

#include <filesystem>
#include <string>
#include <vector>

namespace niminal {
struct Agent;
struct Tool;
} // namespace niminal

namespace niminal::app {

struct Skill {
  std::string name;
  std::string description;
  std::filesystem::path path;
};

std::vector<Skill> discover_skills(const std::filesystem::path& workspace);
std::string load_skill(const std::filesystem::path& workspace, const std::string& name);
niminal::Tool skill_tool(const std::filesystem::path& workspace);
void refresh_skill_tool(niminal::Agent& agent, const std::filesystem::path& workspace);

} // namespace niminal::app
