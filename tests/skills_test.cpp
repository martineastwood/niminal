#include "skills.hpp"
#include "trust.hpp"

#include <niminal/types.hpp>

#include <algorithm>
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
  fs::create_directories(root / "home/.agents/skills/catalog/code-review");
  {
    std::ofstream out(root / "home/.agents/skills/catalog/code-review/SKILL.md");
    out << "---\nname: code-review\ndescription: Review code portably\nlicense: MIT\n---\n\n"
           "Read references/checklist.md before reviewing.\n";
  }
  niminal::app::set_project_resources_trusted(root, false);
  auto untrusted_skills = niminal::app::discover_skills(root);
  if (untrusted_skills.size() != 1 || untrusted_skills[0].name != "code-review") {
    return 1;
  }
  niminal::app::set_project_resources_trusted(root, true);
  auto skills = niminal::app::discover_skills(root);
  if (skills.size() != 2) {
    return 1;
  }
  auto review = std::find_if(skills.begin(), skills.end(),
                             [](const auto& skill) { return skill.name == "review"; });
  auto code_review = std::find_if(skills.begin(), skills.end(),
                                  [](const auto& skill) { return skill.name == "code-review"; });
  if (review == skills.end() || review->description != "Review code carefully" ||
      code_review == skills.end() || code_review->description != "Review code portably") {
    return 1;
  }
  if (niminal::app::load_skill(root, "review").find("Do the review.") == std::string::npos) {
    return 1;
  }
  auto loaded = niminal::app::load_skill(root, "code-review");
  if (loaded.find("Skill directory: ") == std::string::npos ||
      loaded.find("Read references/checklist.md") == std::string::npos) {
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
