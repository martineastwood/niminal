#include "models_dev.hpp"
#include "thinking.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>
#include <vector>

namespace fs = std::filesystem;
using niminal::app::normalize_thinking;
using niminal::app::set_catalog_cache_path;
using niminal::app::snap_to_efforts;
using niminal::app::thinking_body;
using niminal::app::thinking_choices;
using niminal::app::thinking_status;

static int fail(const char* msg) {
  std::cerr << msg << '\n';
  return 1;
}

int main() {
  try {
    if (normalize_thinking(" High ") != "high") {
      return fail("normalize");
    }
  } catch (...) {
    return fail("normalize threw");
  }
  try {
    normalize_thinking("loud");
    return fail("loud should throw");
  } catch (...) {
  }

  if (snap_to_efforts("medium", {"high", "xhigh"}) != "high") {
    return fail("snap medium -> high");
  }
  if (!snap_to_efforts("none", {"high", "xhigh"}).empty()) {
    return fail("none does not snap up");
  }

  auto dir = fs::temp_directory_path() / "niminal-thinking-test";
  fs::remove_all(dir);
  fs::create_directories(dir);
  auto path = dir / "models-dev.json";
  {
    std::ofstream out(path);
    out << R"({
      "openrouter": {"models": {
        "flash": {"reasoning": true,
          "reasoning_options": [{"type": "effort", "values": ["high", "xhigh"]}]},
        "toggle-only": {"reasoning": true,
          "reasoning_options": [{"type": "toggle"}]},
        "dumb": {"reasoning": false}
      }}
    })";
  }
  set_catalog_cache_path(path);

  auto flash = thinking_choices("openrouter", "flash");
  if (flash != std::vector<std::string>{"none", "high", "xhigh"}) {
    return fail("flash choices");
  }
  if (thinking_status("openrouter", "flash", "medium") != "high") {
    return fail("snap status");
  }
  if (thinking_body("openrouter", "flash", "medium")["reasoning"].value("effort", "") != "high") {
    return fail("snap body");
  }
  if (thinking_status("openrouter", "flash", "none") != "off") {
    return fail("none status");
  }
  if (!thinking_body("openrouter", "flash", "none").empty()) {
    return fail("none omits reasoning");
  }
  if (thinking_status("openrouter", "toggle-only", "medium") != "on") {
    return fail("toggle on");
  }
  if (!thinking_body("openrouter", "toggle-only", "medium")["reasoning"].value("enabled", false)) {
    return fail("toggle body");
  }
  if (!thinking_choices("openrouter", "dumb").empty()) {
    return fail("no reasoning");
  }
  if (!thinking_status("openrouter", "dumb", "high").empty()) {
    return fail("unsupported status");
  }

  auto anthro = thinking_choices("anthropic", "claude-sonnet-4-6");
  if (anthro.front() != "none" || anthro.back() != "max") {
    return fail("anthropic choices");
  }
  auto body = thinking_body("anthropic", "claude-sonnet-4-6", "high");
  if (body["thinking"].value("type", "") != "adaptive" ||
      body["output_config"].value("effort", "") != "high") {
    return fail("anthropic adaptive");
  }
  if (thinking_body("anthropic", "claude-sonnet-4-6", "none")["thinking"].value("type", "") !=
      "disabled") {
    return fail("anthropic none");
  }
  if (thinking_body("openai", "gpt-5", "high")["reasoning"].value("effort", "") != "high") {
    return fail("openai effort");
  }
  if (thinking_body("opencode", "x", "high").value("reasoning_effort", "") != "high") {
    return fail("opencode effort");
  }

  fs::remove_all(dir);
  return 0;
}
