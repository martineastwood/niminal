#include "tools.hpp"

#include <cstdio>
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

niminal::Tool* find(std::vector<niminal::Tool>& tools, const char* name) {
  for (auto& t : tools) {
    if (t.name == name) {
      return &t;
    }
  }
  return nullptr;
}

int main() {
  auto tmp = fs::temp_directory_path() / "niminal-tools-test";
  fs::remove_all(tmp);
  fs::create_directories(tmp / "sub");
  fs::create_directories(tmp / "build");
  {
    std::ofstream out(tmp / "a.txt");
    out << "hi\n";
  }
  {
    std::ofstream out(tmp / "build" / "generated.o");
    out << "binary-ish\n";
  }
  Workspace ws(tmp);
  std::atomic<bool> cancel{false};
  auto tools = workspace_tools(ws, &cancel);
  if (!read_only(tools, "read") || !read_only(tools, "grep") || !read_only(tools, "glob") ||
      !read_only(tools, "ls")) {
    std::cerr << "read/grep/glob/ls should be read_only\n";
    return 1;
  }
  if (read_only(tools, "edit") || read_only(tools, "write") || read_only(tools, "bash")) {
    std::cerr << "edit/write/bash should not be read_only\n";
    return 1;
  }
  niminal::Tool* bash = find(tools, "bash");
  niminal::Tool* read = find(tools, "read");
  niminal::Tool* grep = find(tools, "grep");
  niminal::Tool* ls = find(tools, "ls");
  niminal::Tool* glob = find(tools, "glob");
  if ((bash == nullptr) || (read == nullptr) || (grep == nullptr) || (ls == nullptr) ||
      (glob == nullptr)) {
    std::cerr << "missing bash, grep, glob, or ls\n";
    return 1;
  }
  auto listed = ls->run(nlohmann::json{{"path", "."}}).text;
  {
    std::ofstream image(tmp / "screen.png", std::ios::binary);
    image.write("\x89PNG\r\n\x1a\n", 8);
  }
  auto image_read = read->run(nlohmann::json{{"path", "screen.png"}});
  if (image_read.images.size() != 1 || image_read.images[0].value("data", "") != "iVBORw0KGgo=" ||
      image_read.text.find("screen.png") == std::string::npos) {
    std::cerr << "read should return image content\n";
    return 1;
  }
  if (listed.find("sub/") == std::string::npos || listed.find("a.txt") == std::string::npos) {
    std::cerr << "ls should list files and directories\n" << listed << '\n';
    return 1;
  }
  // ls reads the directory itself, so it stays the way to discover entries the
  // file index hides (skip dirs such as build/, plus empty directories).
  if (listed.find("build/") == std::string::npos) {
    std::cerr << "ls should show entries the file index skips\n" << listed << '\n';
    return 1;
  }
  if (glob->run(nlohmann::json{{"pattern", "**/*"}}).text.find("build/") != std::string::npos) {
    std::cerr << "glob should keep honoring the workspace file index\n";
    return 1;
  }
  if (ls->run(nlohmann::json{{"path", "a.txt"}}).text.find("Not a directory") ==
      std::string::npos) {
    std::cerr << "ls on a file should report Not a directory\n";
    return 1;
  }
  auto defaulted = ls->run(nlohmann::json::object()).text;
  if (defaulted.find("a.txt") == std::string::npos) {
    std::cerr << "ls should default to the workspace root\n" << defaulted << '\n';
    return 1;
  }
  auto many = tmp / "many";
  fs::create_directories(many);
  for (int i = 0; i < 205; ++i) {
    char name[16];
    std::snprintf(name, sizeof(name), "f-%03d.txt", i);
    std::ofstream out(many / name);
    out << "x\n";
  }
  auto capped = ls->run(nlohmann::json{{"path", "many"}}).text;
  if (capped.find("[truncated]") == std::string::npos) {
    std::cerr << "ls should mark directories with more than 200 entries\n" << capped << '\n';
    return 1;
  }
  if (capped.find("f-000.txt") == std::string::npos ||
      capped.find("f-199.txt") == std::string::npos ||
      capped.find("f-200.txt") != std::string::npos) {
    std::cerr << "ls should keep the first 200 entries after sorting\n" << capped << '\n';
    return 1;
  }
  auto sed_read = bash->run(nlohmann::json{{"command", "cd .; sed -n 1,1p a.txt"}}).text;
  if (sed_read.find("hi") == std::string::npos) {
    std::cerr << "bash should allow sed file reads\n" << sed_read << '\n';
    return 1;
  }
  auto cr = bash->run(nlohmann::json{{"command", "printf 'hello\\rworld\\n'"}}).text;
  if (cr.find("world") == std::string::npos || cr.find("hello") != std::string::npos) {
    std::cerr << "carriage return should overwrite the current line\n" << cr << '\n';
    return 1;
  }
  std::vector<std::string> snapshots;
  auto streaming = workspace_tools(
      ws, &cancel, [&](const std::string& snapshot) { snapshots.push_back(snapshot); });
  niminal::Tool* streaming_bash = find(streaming, "bash");
  if (streaming_bash == nullptr) {
    std::cerr << "missing streaming bash\n";
    return 1;
  }
  auto streamed = streaming_bash->run(nlohmann::json{{"command", "printf 'one\\ntwo\\n'"}}).text;
  if (snapshots.empty() || snapshots.back().find("two") == std::string::npos ||
      streamed.find("two") == std::string::npos) {
    std::cerr << "bash should emit output snapshots while running\n";
    return 1;
  }
  if (bash->run(nlohmann::json{{"command", "printenv NIMINAL_SESSION_ID || echo unset"}})
          .text.find("unset") == std::string::npos) {
    std::cerr << "bash should export nothing without a shell env provider\n";
    return 1;
  }
  std::string session_id = "sess-1";
  niminal::app::ShellEnvFn env_fn = [&] {
    return niminal::app::ShellEnv{{"NIMINAL_SESSION_ID", session_id},
                                  {"NIMINAL_SESSION_FILE", "s.jsonl"},
                                  {"NIMINAL_PROVIDER", "openrouter"},
                                  {"NIMINAL_MODEL", "test/model"},
                                  {"NIMINAL_REASONING_LEVEL", "high"}};
  };
  auto env_tools = workspace_tools(ws, &cancel, {}, &env_fn);
  niminal::Tool* env_bash = find(env_tools, "bash");
  if (env_bash == nullptr) {
    std::cerr << "missing env bash\n";
    return 1;
  }
  auto exported =
      env_bash
          ->run(nlohmann::json{{"command",
                                "echo \"$NIMINAL_SESSION_ID|$NIMINAL_SESSION_FILE|"
                                "$NIMINAL_PROVIDER|$NIMINAL_MODEL|$NIMINAL_REASONING_LEVEL\""}})
          .text;
  if (exported.find("sess-1") == std::string::npos ||
      exported.find("s.jsonl") == std::string::npos ||
      exported.find("openrouter") == std::string::npos ||
      exported.find("test/model") == std::string::npos ||
      exported.find("high") == std::string::npos) {
    std::cerr << "bash should export the session env block\n" << exported << '\n';
    return 1;
  }
  session_id = "sess-2";
  auto refreshed = env_bash->run(nlohmann::json{{"command", "printenv NIMINAL_SESSION_ID"}}).text;
  if (refreshed.find("sess-2") == std::string::npos) {
    std::cerr << "bash should re-evaluate the shell env per invocation\n" << refreshed << '\n';
    return 1;
  }
  auto parallel =
      std::async(std::launch::async, [&] { return grep->run(nlohmann::json{{"pattern", "hi"}}); });
  auto parallel2 = std::async(
      std::launch::async, [&] { return grep->run(nlohmann::json{{"pattern", "missing-xyz"}}); });
  if (parallel.get().text.find("a.txt") == std::string::npos) {
    std::cerr << "grep should find a.txt\n";
    return 1;
  }
  if (parallel2.get().text.find("No matches") == std::string::npos) {
    std::cerr << "grep should report no matches\n";
    return 1;
  }
  fs::remove_all(tmp);
  return 0;
}
