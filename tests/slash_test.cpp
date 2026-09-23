#include "slash.hpp"

#include "extensions.hpp"
#include "session.hpp"

#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>

int main() {
  using niminal::app::parse_user_bash;

  auto fail = [](const char* msg) {
    std::cerr << msg << '\n';
    return 1;
  };

  if (parse_user_bash("hello")) {
    return fail("plain text");
  }
  if (parse_user_bash("!")) {
    return fail("bare !");
  }
  if (parse_user_bash("!!")) {
    return fail("bare !!");
  }
  if (parse_user_bash("!   ")) {
    return fail("! with spaces only");
  }

  auto single = parse_user_bash("! git status");
  if (!single || single->exclude_from_context || single->command != "git status") {
    return fail("! git status");
  }

  auto hidden = parse_user_bash("!!pwd");
  if (!hidden || !hidden->exclude_from_context || hidden->command != "pwd") {
    return fail("!!pwd");
  }

  auto spaced = parse_user_bash("!!  echo hi");
  if (!spaced || !spaced->exclude_from_context || spaced->command != "echo hi") {
    return fail("!! echo hi");
  }

  using niminal::app::is_extension_slash;
  using niminal::app::skill_slash_error;
  if (skill_slash_error("/tmp", "/help")) {
    return fail("non-skill slash");
  }
  if (!skill_slash_error("/tmp", "/skill:missing")) {
    return fail("unknown skill");
  }
  if (is_extension_slash(nullptr, "/custom")) {
    return fail("null extensions");
  }

  const auto root = std::filesystem::temp_directory_path() / "niminal-slash-models-test";
  std::filesystem::remove_all(root);
  std::filesystem::create_directories(root / ".niminal");
  std::ofstream(root / ".niminal" / "models.json") << R"({"models":[
    {"provider":"local","name":"coding","runtime":"llamacpp","model":"local-coding","api_url":"http://localhost/v1/chat/completions","context_window":32768},
    {"provider":"foundry","name":"coding","model":"deployment-a","api_url":"https://example.test/openai/responses?api-version=1"},
    {"provider":"foundry","name":"fast","model":"deployment-b","api_url":"https://example.test/openai/responses?api-version=2"}
  ]})";
  const char* previous = std::getenv("HOME");
  const std::string saved = previous == nullptr ? "" : previous;
  setenv("HOME", root.c_str(), 1);
  const niminal::app::Session session;
  auto foundry = niminal::app::slash_suggestions("/model ", root, root.string(), "foundry", "", {},
                                                 {}, session);
  auto local =
      niminal::app::slash_suggestions("/model ", root, root.string(), "local", "", {}, {}, session);
  if (previous == nullptr) {
    unsetenv("HOME");
  } else {
    setenv("HOME", saved.c_str(), 1);
  }
  std::filesystem::remove_all(root);
  if (foundry.size() != 2 || foundry[0].fill != "/model coding" ||
      foundry[1].fill != "/model fast" || local.size() != 1 || local[0].fill != "/model coding") {
    return fail("model suggestions should use the active provider's entries");
  }

  return 0;
}
