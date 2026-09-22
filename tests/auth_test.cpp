#include "auth.hpp"

#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>

namespace fs = std::filesystem;

int main() {
  const auto root = fs::temp_directory_path() / "niminal-auth-test";
  fs::remove_all(root);
  fs::create_directories(root);
  const auto path = root / "auth.json";

  std::ofstream(path) << R"({
    "openai": {"key": "$NIMINAL_TEST_OPENAI_KEY"},
    "anthropic": "literal-anthropic-key",
    "google": {"key": "${NIMINAL_TEST_GOOGLE_KEY}"},
    "invalid": {"key": 42}
  })";
  setenv("NIMINAL_TEST_OPENAI_KEY", "custom-openai-key", 1);
  setenv("NIMINAL_TEST_GOOGLE_KEY", "custom-google-key", 1);

  if (niminal::app::read_auth_key("openai", path) != "custom-openai-key") {
    std::cerr << "environment reference was not resolved\n";
    return 1;
  }
  if (niminal::app::read_auth_key("anthropic", path) != "literal-anthropic-key") {
    std::cerr << "literal key was not loaded\n";
    return 1;
  }
  if (niminal::app::read_auth_key("google", path) != "custom-google-key") {
    std::cerr << "braced environment reference was not resolved\n";
    return 1;
  }
  if (!niminal::app::read_auth_key("invalid", path).empty() ||
      !niminal::app::read_auth_key("missing", path).empty()) {
    std::cerr << "invalid auth entry should be empty\n";
    return 1;
  }

  unsetenv("NIMINAL_TEST_OPENAI_KEY");
  if (!niminal::app::read_auth_key("openai", path).empty()) {
    std::cerr << "missing environment reference should be empty\n";
    return 1;
  }
  unsetenv("NIMINAL_TEST_GOOGLE_KEY");
  fs::remove_all(root);
  return 0;
}
