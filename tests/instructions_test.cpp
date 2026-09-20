#include "instructions.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>

namespace fs = std::filesystem;

static int fail(const char* msg) {
  std::cerr << msg << '\n';
  return 1;
}

int main() {
  auto dir = fs::temp_directory_path() / "niminal-instructions-test";
  fs::remove_all(dir);
  fs::create_directories(dir / "src");
  {
    std::ofstream out(dir / "AGENTS.md");
    out << "from-root\n";
  }
  {
    std::ofstream out(dir / "src" / "AGENTS.md");
    out << "from-src\n";
  }

  auto text = niminal::app::load_project_instructions(dir);
  if (text.find("from-root") == std::string::npos)
    return fail("project instructions should include root AGENTS.md");
  if (text.find("from-src") != std::string::npos)
    return fail("system prefix should not include nested AGENTS.md");

  auto scoped = niminal::app::load_scoped_instructions(dir, dir / "src" / "a.cpp");
  if (scoped.find("from-src") == std::string::npos)
    return fail("read-scoped instructions should include src/AGENTS.md");
  if (scoped.find("from-root") != std::string::npos)
    return fail("scoped instructions should skip files already in the prefix");

  fs::remove_all(dir);
  return 0;
}
