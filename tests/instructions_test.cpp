#include "instructions.hpp"
#include "trust.hpp"

#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>

namespace fs = std::filesystem;

static int fail(const char* msg) {
  std::cerr << msg << '\n';
  return 1;
}

static void write(const fs::path& path, const std::string& text) {
  fs::create_directories(path.parent_path());
  std::ofstream out(path);
  out << text;
}

int main() {
  auto root = fs::temp_directory_path() / "niminal-instructions-test";
  auto home = root / "home";
  auto workspace = root / "workspace";
  fs::remove_all(root);
  fs::create_directories(workspace / "src");
  setenv("HOME", home.c_str(), 1);
  niminal::app::set_context_files_enabled(true);
  niminal::app::set_project_resources_trusted(workspace, true);

  write(workspace / "AGENTS.md", "from-root\n");
  write(workspace / "src" / "AGENTS.md", "from-src\n");

  auto text = niminal::app::load_project_instructions(workspace);
  if (text.find("from-root") == std::string::npos) {
    return fail("project instructions should include root AGENTS.md");
  }
  if (text.find("from-src") != std::string::npos) {
    return fail("system prefix should not include nested AGENTS.md");
  }

  auto scoped = niminal::app::load_scoped_instructions(workspace, workspace / "src" / "a.cpp");
  if (scoped.find("from-src") == std::string::npos) {
    return fail("read-scoped instructions should include src/AGENTS.md");
  }
  if (scoped.find("from-root") != std::string::npos) {
    return fail("scoped instructions should skip files already in the prefix");
  }

  fs::remove(workspace / "AGENTS.md");
  write(workspace / "CLAUDE.md", "from-claude\n");
  auto claude = niminal::app::load_project_instructions(workspace);
  if (claude.find("from-claude") == std::string::npos) {
    return fail("CLAUDE.md should load when AGENTS.md is absent");
  }

  write(workspace / "AGENTS.md", "from-agents\n");
  auto agents = niminal::app::load_project_instructions(workspace);
  if (agents.find("from-agents") == std::string::npos ||
      agents.find("from-claude") != std::string::npos) {
    return fail("AGENTS.md should win over CLAUDE.md");
  }

  write(workspace / "AGENTS.override.md", "from-override\n");
  auto override_text = niminal::app::load_project_instructions(workspace);
  if (override_text.find("from-override") == std::string::npos ||
      override_text.find("from-agents") != std::string::npos) {
    return fail("AGENTS.override.md should win over AGENTS.md");
  }

  fs::remove(workspace / "AGENTS.override.md");
  write(workspace / ".niminal" / "SYSTEM.md", "custom-system\n");
  write(workspace / ".niminal" / "APPEND_SYSTEM.md", "custom-append\n");
  write(home / ".niminal" / "SYSTEM.md", "global-system\n");
  write(home / ".niminal" / "APPEND_SYSTEM.md", "global-append\n");

  if (niminal::app::load_system_prompt(workspace) != "custom-system\n") {
    return fail("trusted project SYSTEM.md should load");
  }
  if (niminal::app::load_append_system_prompt(workspace) != "custom-append\n") {
    return fail("trusted project APPEND_SYSTEM.md should load");
  }

  niminal::app::set_project_resources_trusted(workspace, false);
  if (niminal::app::load_system_prompt(workspace) != "global-system\n") {
    return fail("untrusted project should fall back to global SYSTEM.md");
  }
  if (niminal::app::load_append_system_prompt(workspace) != "global-append\n") {
    return fail("untrusted project should fall back to global APPEND_SYSTEM.md");
  }

  fs::remove(workspace / ".niminal" / "SYSTEM.md");
  fs::remove(workspace / ".niminal" / "APPEND_SYSTEM.md");
  niminal::app::set_project_resources_trusted(workspace, true);
  if (niminal::app::load_system_prompt(workspace) != "global-system\n") {
    return fail("missing project SYSTEM.md should fall back to global");
  }
  if (niminal::app::load_append_system_prompt(workspace) != "global-append\n") {
    return fail("missing project APPEND_SYSTEM.md should fall back to global");
  }

  fs::remove(home / ".niminal" / "SYSTEM.md");
  fs::remove(home / ".niminal" / "APPEND_SYSTEM.md");
  if (!niminal::app::load_system_prompt(workspace).empty() ||
      !niminal::app::load_append_system_prompt(workspace).empty()) {
    return fail("missing system prompt files should return empty");
  }

  niminal::app::set_context_files_enabled(false);
  if (!niminal::app::load_project_instructions(workspace).empty()) {
    return fail("--no-context-files should skip project instructions");
  }
  if (!niminal::app::load_scoped_instructions(workspace, workspace / "src" / "a.cpp").empty()) {
    return fail("--no-context-files should skip scoped instructions");
  }

  fs::remove_all(root);
  return 0;
}
