#include "prompts.hpp"
#include "trust.hpp"

#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>

namespace fs = std::filesystem;

int main() {
  auto root = fs::temp_directory_path() / "niminal-prompts-test";
  fs::remove_all(root);
  fs::create_directories(root / "home/.agents/prompts");
  fs::create_directories(root / "home/.niminal/prompts");
  fs::create_directories(root / "workspace/.agents/prompts");
  fs::create_directories(root / "workspace/.niminal/prompts");
  setenv("HOME", (root / "home").c_str(), 1);

  {
    std::ofstream out(root / "home/.agents/prompts/shared.md");
    out << "Shared $@.";
  }
  {
    std::ofstream out(root / "home/.niminal/prompts/review.md");
    out << "---\ndescription: \"Review the target.\"\n---\nReview $ARGUMENTS carefully.";
  }
  {
    std::ofstream out(root / "workspace/.agents/prompts/shared.md");
    out << "Project $@.";
  }
  {
    std::ofstream out(root / "workspace/.niminal/prompts/fallback.md");
    out << "First useful line\nmore details";
  }

  niminal::app::set_project_resources_trusted(root / "workspace", false);
  if (niminal::app::discover_prompts(root / "workspace").size() != 2) return 1;
  niminal::app::set_project_resources_trusted(root / "workspace", true);
  auto prompts = niminal::app::discover_prompts(root / "workspace");
  if (prompts.size() != 3) return 1;
  auto review = niminal::app::load_prompt(root / "workspace", "REVIEW");
  if (!review || review->description != "Review the target.") return 1;
  if (niminal::app::expand_prompt(root / "workspace", "review", "src/foo.cpp") !=
      "Review src/foo.cpp carefully.")
    return 1;
  if (niminal::app::expand_prompt(root / "workspace", "shared", "request") !=
      "Project request.")
    return 1;
  auto fallback = niminal::app::load_prompt(root / "workspace", "fallback");
  if (!fallback || fallback->description != "First useful line") return 1;

  fs::create_directories(root / "workspace/.niminal/prompts/nested");
  {
    std::ofstream out(root / "workspace/.niminal/prompts/ignored.txt");
    out << "ignored";
  }
  {
    std::ofstream out(root / "workspace/.niminal/prompts/empty.md");
  }
  fs::remove_all(root);
}
