#include "skills.hpp"
#include "config.hpp"
#include "trust.hpp"

#include <niminal/agent.hpp>
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

std::string unquote(std::string value) {
  while (!value.empty() && value.front() == ' ') {
    value.erase(value.begin());
  }
  if (value.size() >= 2 && ((value.front() == '"' && value.back() == '"') ||
                            (value.front() == '\'' && value.back() == '\''))) {
    return value.substr(1, value.size() - 2);
  }
  return value;
}

struct SkillMetadata {
  std::string name;
  std::string description;
};

SkillMetadata metadata_of(const fs::path& path) {
  SkillMetadata metadata;
  std::istringstream in(read_file(path));
  std::string line;
  if (!std::getline(in, line)) {
    return metadata;
  }
  if (!line.empty() && line.back() == '\r') {
    line.pop_back();
  }
  if (line != "---") {
    return metadata;
  }
  while (std::getline(in, line)) {
    if (!line.empty() && line.back() == '\r') {
      line.pop_back();
    }
    if (line == "---") {
      break;
    }
    const auto colon = line.find(':');
    if (colon == std::string::npos) {
      continue;
    }
    auto key = line.substr(0, colon);
    while (!key.empty() && key.back() == ' ') {
      key.pop_back();
    }
    auto value = unquote(line.substr(colon + 1));
    if (key == "name") {
      metadata.name = std::move(value);
    } else if (key == "description") {
      metadata.description = std::move(value);
    }
  }
  return metadata;
}

void add_dir(std::map<std::string, Skill>& skills, const fs::path& dir) {
  std::error_code ec;
  if (!fs::is_directory(dir, ec)) {
    return;
  }
  std::vector<fs::path> manifests;
  for (const auto& entry :
       fs::recursive_directory_iterator(dir, fs::directory_options::skip_permission_denied, ec)) {
    if (!entry.is_regular_file(ec) || entry.path().filename() != "SKILL.md") {
      continue;
    }
    manifests.push_back(entry.path());
  }
  std::sort(manifests.begin(), manifests.end());
  for (const auto& path : manifests) {
    auto metadata = metadata_of(path);
    auto name =
        metadata.name.empty() ? path.parent_path().filename().string() : std::move(metadata.name);
    if (!name.empty()) {
      const auto key = name;
      skills[key] = {std::move(name), std::move(metadata.description), path};
    }
  }
}

} // namespace

std::vector<Skill> discover_skills(const fs::path& workspace) {
  std::map<std::string, Skill> found;
  try {
    const auto global = config_path().parent_path();
    add_dir(found, global.parent_path() / ".agents" / "skills");
    add_dir(found, global / "skills");
  } catch (...) {
  }
  if (project_resources_trusted(workspace)) {
    add_dir(found, workspace / ".agent" / "skills");
    add_dir(found, workspace / ".agents" / "skills");
    add_dir(found, workspace / ".niminal" / "skills");
  }
  std::vector<Skill> out;
  for (auto& [_, skill] : found) {
    out.push_back(std::move(skill));
  }
  return out;
}

std::string load_skill(const fs::path& workspace, const std::string& name) {
  auto skills = discover_skills(workspace);
  auto it = std::find_if(skills.begin(), skills.end(),
                         [&](const Skill& skill) { return skill.name == name; });
  if (it == skills.end()) {
    return "Skill not found: " + name;
  }
  auto text = read_file(it->path);
  if (text.empty()) {
    return "Skill is empty: " + name;
  }
  return "Skill directory: " + it->path.parent_path().string() + "\n\n" + text;
}

niminal::Tool skill_tool(const fs::path& workspace) {
  auto skills = discover_skills(workspace);
  std::string description = "Load a skill's instructions when its expertise helps the task.";
  if (!skills.empty()) {
    description += " Available skills:";
    for (const auto& skill : skills) {
      description += "\n- " + skill.name;
      if (!skill.description.empty()) {
        description += ": " + skill.description;
      }
    }
  }
  return {"skill", std::move(description),
          json{{"type", "object"},
               {"properties", {{"name", {{"type", "string"}}}}},
               {"required", json::array({"name"})}},
          [workspace](const json& input) {
            return load_skill(workspace, input.at("name").get<std::string>());
          },
          true};
}

void refresh_skill_tool(niminal::Agent& agent, const std::filesystem::path& workspace) {
  for (auto& tool : agent.tools) {
    if (tool.name == "skill") {
      tool = skill_tool(workspace);
      return;
    }
  }
}

} // namespace niminal::app
