#include "permissions.hpp"
#include "trust.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>
#include <cstdlib>

namespace fs = std::filesystem;
using niminal::ToolCall;
using niminal::app::PermissionCheck;
using niminal::app::PermissionDecision;
using niminal::app::PermissionPolicy;

int main() {
  auto root = fs::temp_directory_path() / "niminal-permissions-test";
  auto home = root / "home";
  fs::remove_all(root);
  fs::create_directories(root / ".niminal");
  fs::create_directories(home);
  setenv("HOME", home.c_str(), 1);
  niminal::app::set_project_resources_trusted(root, true);

  ToolCall read{"1", "read", R"({"path":"README.md"})"};
  ToolCall bash{"2", "bash", R"({"command":"npm   test"})"};
  ToolCall dangerous{"3", "bash", R"({"command":"npm test && rm -rf build"})"};
  ToolCall git{"4", "git", R"({"status":true})"};

  if (niminal::app::permission_key(bash) != "bash:npm test" ||
      niminal::app::permission_description(bash) != "npm test") {
    std::cerr << "bash permission key mismatch\n";
    return 1;
  }
  if (!niminal::app::dangerous_command("echo rm") ||
      niminal::app::can_remember(dangerous)) {
    std::cerr << "dangerous command policy mismatch\n";
    return 1;
  }

  PermissionPolicy policy(root);
  if (policy.check(read) != PermissionCheck::allow ||
      policy.check(bash) != PermissionCheck::ask ||
      policy.check(git) != PermissionCheck::ask) {
    std::cerr << "default permission checks mismatch\n";
    return 1;
  }
  policy.remember(bash, PermissionDecision::allow_session);
  if (policy.check(bash) != PermissionCheck::allow) {
    std::cerr << "session grant was not applied\n";
    return 1;
  }
  policy.remember(git, PermissionDecision::allow_project);
  PermissionPolicy reloaded(root);
  if (reloaded.check(git) != PermissionCheck::allow ||
      reloaded.check(dangerous) != PermissionCheck::ask) {
    std::cerr << "project or dangerous grant mismatch\n";
    return 1;
  }
  reloaded.clear_project();
  PermissionPolicy cleared(root);
  if (cleared.check(git) != PermissionCheck::ask) {
    std::cerr << "clear should remove project grants\n";
    return 1;
  }

  fs::remove_all(root);
  return 0;
}
