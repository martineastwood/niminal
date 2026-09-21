#include "tools.hpp"

#include <filesystem>
#include <fstream>
#include <future>
#include <iostream>
#include <string>
#include <vector>

namespace fs = std::filesystem;
using niminal::app::Workspace;
using niminal::app::workspace_tools;

bool read_only(const std::vector<niminal::Tool>& tools, const char* name) {
  for (const auto& t : tools) {
    if (t.name == name) {
      return t.read_only;
    }
  }
  return false;
}

int main() {
  auto tmp = fs::temp_directory_path() / "niminal-tools-test";
  fs::remove_all(tmp);
  fs::create_directories(tmp);
  {
    std::ofstream out(tmp / "a.txt");
    out << "hi\n";
  }
  Workspace ws(tmp);
  std::atomic<bool> cancel{false};
  auto tools = workspace_tools(ws, &cancel);
  if (!read_only(tools, "read") || !read_only(tools, "grep") || !read_only(tools, "glob")) {
    std::cerr << "read/grep/glob should be read_only\n";
    return 1;
  }
  if (read_only(tools, "edit") || read_only(tools, "write") || read_only(tools, "bash")) {
    std::cerr << "edit/write/bash should not be read_only\n";
    return 1;
  }
  niminal::Tool* bash = nullptr;
  niminal::Tool* grep = nullptr;
  for (auto& t : tools) {
    if (t.name == "bash") {
      bash = &t;
    }
    if (t.name == "grep") {
      grep = &t;
    }
  }
  if ((bash == nullptr) || (grep == nullptr)) {
    std::cerr << "missing bash or grep\n";
    return 1;
  }
  auto cr = bash->run(nlohmann::json{{"command", "printf 'hello\\rworld\\n'"}});
  if (cr.find("world") == std::string::npos || cr.find("hello") != std::string::npos) {
    std::cerr << "carriage return should overwrite the current line\n" << cr << '\n';
    return 1;
  }
  std::vector<std::string> snapshots;
  auto streaming = workspace_tools(
      ws, &cancel, [&](const std::string& snapshot) { snapshots.push_back(snapshot); });
  niminal::Tool* streaming_bash = nullptr;
  for (auto& t : streaming) {
    if (t.name == "bash") {
      streaming_bash = &t;
    }
  }
  if (streaming_bash == nullptr) {
    std::cerr << "missing streaming bash\n";
    return 1;
  }
  auto streamed = streaming_bash->run(nlohmann::json{{"command", "printf 'one\\ntwo\\n'"}});
  if (snapshots.empty() || snapshots.back().find("two") == std::string::npos ||
      streamed.find("two") == std::string::npos) {
    std::cerr << "bash should emit output snapshots while running\n";
    return 1;
  }
  auto parallel =
      std::async(std::launch::async, [&] { return grep->run(nlohmann::json{{"pattern", "hi"}}); });
  auto parallel2 = std::async(
      std::launch::async, [&] { return grep->run(nlohmann::json{{"pattern", "missing-xyz"}}); });
  if (parallel.get().find("a.txt") == std::string::npos) {
    std::cerr << "grep should find a.txt\n";
    return 1;
  }
  if (parallel2.get().find("No matches") == std::string::npos) {
    std::cerr << "grep should report no matches\n";
    return 1;
  }
  fs::remove_all(tmp);
  return 0;
}
