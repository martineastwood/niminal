#include "skills.hpp"
#include "config.hpp"

#include <niminal/types.hpp>

#include <algorithm>
#include <fstream>
#include <map>
#include <sstream>
#include <stdexcept>

namespace niminal::app {
namespace fs = std::filesystem;
using niminal::json;

namespace {

std::string read_file(const fs::path& path) {
  std::ifstream in(path);
  std::ostringstream out;
  out << in.rdbuf();
  return out.str();
}

std::string description_of(const fs::path& path) {
  std::istringstream in(read_file(path));
  std::string line;
  if (!std::getline(in, line) || line != "---") return {};
  while (std::getline(in, line) && line != "---") {
    if (!line.starts_with("description:")) continue;
    auto value = line.substr(12);
    while (!value.empty() && value.front() == ' ') value.erase(value.begin());
    if (value.size() >= 2 && value.front() == '"' && value.back() == '"')
      value = value.substr(1, value.size() - 2);
    return value;
  }
  return {};
}

void add_dir(std::map<std::string, Skill>& skills, const fs::path& dir) {
  std::error_code ec;
  if (!fs::is_directory(dir, ec)) return;
  for (const auto& entry : fs::directory_iterator(dir, ec)) {
    auto path = entry.path() / "SKILL.md";
    if (!entry.is_directory(ec) || !fs::is_regular_file(path, ec)) continue;
    auto name = entry.path().filename().string();
    skills[name] = {name, description_of(path), path};
  }
}

}  // namespace

std::vector<Skill> discover_skills(const fs::path& workspace) {
  std::map<std::string, Skill> found;
  try {
    add_dir(found, config_path().parent_path() / "skills");
  } catch (...) {
  }
  add_dir(found, workspace / ".niminal" / "skills");
  std::vector<Skill> out;
  for (auto& [_, skill] : found) out.push_back(std::move(skill));
  return out;
}

std::string load_skill(const fs::path& workspace, const std::string& name) {
  auto skills = discover_skills(workspace);
  auto it = std::find_if(skills.begin(), skills.end(), [&](const Skill& skill) {
    return skill.name == name;
  });
  if (it == skills.end()) return "Skill not found: " + name;
  auto text = read_file(it->path);
  return text.empty() ? "Skill is empty: " + name : text;
}

niminal::Tool skill_tool(const fs::path& workspace) {
  auto skills = discover_skills(workspace);
  std::string description = "Load a skill's instructions when its expertise helps the task.";
  if (!skills.empty()) {
    description += " Available skills:";
    for (const auto& skill : skills) {
      description += "\n- " + skill.name;
      if (!skill.description.empty()) description += ": " + skill.description;
    }
  }
  return {"skill",
          std::move(description),
          json{{"type", "object"},
               {"properties", {{"name", {{"type", "string"}}}}},
               {"required", json::array({"name"})}},
          [workspace](const json& input) {
            return load_skill(workspace, input.at("name").get<std::string>());
          }};
}

}  // namespace niminal::app
