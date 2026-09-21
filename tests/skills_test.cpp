#include "skills.hpp"
#include "trust.hpp"

#include <niminal/types.hpp>

#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>

namespace fs = std::filesystem;

int main() {
  auto root = fs::temp_directory_path() / "niminal-skills-test";
  fs::remove_all(root);
  fs::create_directories(root / "home");
  setenv("HOME", (root / "home").c_str(), 1);
  fs::create_directories(root / ".niminal/skills/review");
  {
    std::ofstream out(root / ".niminal/skills/review/SKILL.md");
    out << "---\ndescription: Review code carefully\n---\n\nDo the review.\n";
  }
  niminal::app::set_project_resources_trusted(root, false);
  if (!niminal::app::discover_skills(root).empty()) {
    return 1;
  }
  niminal::app::set_project_resources_trusted(root, true);
  auto skills = niminal::app::discover_skills(root);
  if (skills.size() != 1 || skills[0].name != "review" ||
      skills[0].description != "Review code carefully") {
    return 1;
  }
  if (niminal::app::load_skill(root, "review").find("Do the review.") == std::string::npos) {
    return 1;
  }
  if (niminal::app::load_skill(root, "missing") != "Skill not found: missing") {
    return 1;
  }
  if (!niminal::app::skill_tool(root).read_only) {
    return 1;
  }
  fs::remove_all(root);
}
