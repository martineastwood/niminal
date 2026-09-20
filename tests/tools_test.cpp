#include "tools.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>

namespace fs = std::filesystem;
using niminal::app::Workspace;
using niminal::app::workspace_tools;

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
  niminal::Tool* bash = nullptr;
  for (auto& t : tools)
    if (t.name == "bash") bash = &t;
  if (!bash) {
    std::cerr << "missing bash\n";
    return 1;
  }
  auto out = bash->run(nlohmann::json{{"command", "echo niminal-ok"}});
  if (out.find("niminal-ok") == std::string::npos) {
    std::cerr << out << '\n';
    return 1;
  }
  fs::remove_all(tmp);
  return 0;
}
