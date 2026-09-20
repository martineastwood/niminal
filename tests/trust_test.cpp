#include "trust.hpp"

#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>

namespace fs = std::filesystem;
using niminal::app::TrustOverride;

int main() {
  auto root = fs::temp_directory_path() / "niminal-trust-test";
  auto home = root / "home";
  auto workspace = root / "workspace";
  auto child = workspace / "packages" / "app";
  fs::remove_all(root);
  fs::create_directories(home);
  fs::create_directories(child / ".niminal" / "skills" / "review");
  fs::create_directories(child / ".niminal" / "prompts");
  fs::create_directories(child / ".niminal" / "extensions" / "guard");
  std::ofstream(child / ".niminal" / "permissions.json") << "{\"allow\":[]}";
  std::ofstream(child / ".niminal" / "skills" / "review" / "SKILL.md") << "review";
  std::ofstream(child / ".niminal" / "prompts" / "review.md") << "review";
  std::ofstream(child / ".niminal" / "extensions" / "guard" /
                "extension.json") << "{}";
  setenv("HOME", home.c_str(), 1);

  auto resources = niminal::app::project_trust_resources(child);
  if (resources.size() != 4) {
    std::cerr << "unexpected trust resource count\n";
    return 1;
  }
  auto pending = niminal::app::resolve_project_trust(child);
  if (!pending.required || !pending.prompt || pending.trusted) {
    std::cerr << "unseen project should require trust\n";
    return 1;
  }
  niminal::app::set_project_resources_trusted(child, false);
  if (niminal::app::project_resources_trusted(child)) {
    std::cerr << "trust override was not applied\n";
    return 1;
  }
  auto approved = niminal::app::resolve_project_trust(child, TrustOverride::approve);
  if (!approved.trusted || approved.prompt) {
    std::cerr << "approve override mismatch\n";
    return 1;
  }
  niminal::app::save_project_trust(workspace, true);
  auto inherited = niminal::app::resolve_project_trust(child);
  if (!inherited.trusted || inherited.prompt) {
    std::cerr << "parent trust was not inherited\n";
    return 1;
  }

  fs::remove_all(root);
  return 0;
}
