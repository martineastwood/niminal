#include "workspace.hpp"

#include <cassert>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>

namespace fs = std::filesystem;
using niminal::app::Workspace;
using niminal::app::WorkspaceError;

int main() {
  auto tmp = fs::temp_directory_path() / "niminal-ws-test";
  fs::remove_all(tmp);
  fs::create_directories(tmp / "src");
  {
    std::ofstream out(tmp / "src" / "a.txt");
    out << "hello\n";
  }

  Workspace ws(tmp);
  auto inside = ws.resolve("src/a.txt");
  if (ws.relative(inside) != "src/a.txt") {
    std::cerr << "relative path mismatch\n";
    return 1;
  }

  bool escaped = false;
  try {
    ws.resolve("../outside.txt");
  } catch (const WorkspaceError&) {
    escaped = true;
  }
  if (!escaped) {
    std::cerr << "cwd jail failed for relative escape\n";
    return 1;
  }

  escaped = false;
  try {
    ws.resolve("/etc/passwd");
  } catch (const WorkspaceError&) {
    escaped = true;
  }
  if (!escaped) {
    std::cerr << "cwd jail failed for absolute path\n";
    return 1;
  }

  fs::create_directories(tmp / "build");
  {
    std::ofstream out(tmp / ".gitignore");
    out << "/build/\n";
  }
  {
    std::ofstream out(tmp / "src" / "keep.cpp");
    out << "int main() {}\n";
  }
  {
    std::ofstream out(tmp / "build" / "junk.cpp");
    out << "ignored\n";
  }
  if (std::system(("git -C " + tmp.string() +
                   " init -q && git -C " + tmp.string() +
                   " config user.email t@t && git -C " + tmp.string() +
                   " config user.name t")
                      .c_str()) != 0) {
    std::cerr << "git init failed\n";
    fs::remove_all(tmp);
    return 1;
  }
  Workspace listed(tmp);
  auto files = listed.list_files();
  bool saw_src = false;
  bool saw_build = false;
  for (const auto& f : files) {
    if (f == "src/keep.cpp") saw_src = true;
    if (f.find("build/") != std::string::npos) saw_build = true;
  }
  if (!saw_src) {
    std::cerr << "list_files should include untracked src file\n";
    fs::remove_all(tmp);
    return 1;
  }
  if (saw_build) {
    std::cerr << "list_files should honor gitignore build/\n";
    fs::remove_all(tmp);
    return 1;
  }

  fs::remove_all(tmp);
  return 0;
}
