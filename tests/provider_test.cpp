#include "provider.hpp"

#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>

using niminal::app::apply_provider;
using niminal::app::Config;
using niminal::app::normalize_config;
using niminal::app::select_provider;

static int fail(const char* msg) {
  std::cerr << msg << '\n';
  return 1;
}

int main() {
  if ((niminal::find_provider("anthropic") == nullptr) ||
      (niminal::find_provider("GOOGLE") == nullptr) ||
      (niminal::find_provider("codex") != nullptr) ||
      (niminal::find_provider("gemini") != nullptr)) {
    return fail("find_provider");
  }
  auto names = niminal::provider_names();
  if (names.find("openrouter") == std::string::npos || names.find("mistral") == std::string::npos) {
    return fail("provider_names");
  }
  if (niminal::infer_provider("https://api.anthropic.com/v1/messages") != "anthropic" ||
      niminal::infer_provider("https://generativelanguage.googleapis.com/v1beta") != "google" ||
      niminal::infer_provider("https://opencode.ai/zen/go/v1/chat/completions") != "opencode") {
    return fail("infer_provider");
  }

  Config cfg;
  if (auto result = select_provider(cfg, "anthropic"); !result) {
    return fail(result.error().what());
  }
  if (cfg.provider != "anthropic" || cfg.model != "claude-sonnet-4-6") {
    return fail("select anthropic default model");
  }
  if (cfg.last_models["openrouter"] != "openai/gpt-4o-mini") {
    return fail("remember previous model");
  }
  cfg.model = "claude-opus-4-6";
  if (auto result = select_provider(cfg, "openai"); !result) {
    return fail(result.error().what());
  }
  if (cfg.model != "gpt-5") {
    return fail("openai default");
  }
  if (auto result = select_provider(cfg, "anthropic"); !result) {
    return fail(result.error().what());
  }
  if (cfg.model != "claude-opus-4-6") {
    return fail("restore last model");
  }
  auto codex = select_provider(cfg, "codex");
  if (codex) {
    return fail("codex should fail");
  }
  if (std::string(codex.error().what()).find("codex") == std::string::npos) {
    return fail("codex error");
  }
  if (auto bad = select_provider(cfg, "nope"); bad) {
    return fail("unknown should fail");
  }

  niminal::Agent agent;
  apply_provider(agent, cfg);
  if (agent.provider != "anthropic" || agent.api_url.find("/v1/messages") == std::string::npos ||
      agent.extra_headers["anthropic-version"] != "2023-06-01" || !agent.apply_cache ||
      agent.session_routing || agent.stream_usage || agent.key_hint != "ANTHROPIC_API_KEY") {
    return fail("apply_provider anthropic");
  }

  Config stale;
  stale.provider = "anthropic";
  stale.model = "claude-haiku-4-5";
  stale.api_url = "https://api.anthropic.com/v1/chat/completions";
  normalize_config(stale);
  apply_provider(agent, stale);
  if (agent.api_url.find("/v1/messages") == std::string::npos) {
    return fail("stale api_url should normalize to provider endpoint");
  }

  const auto auth_root = std::filesystem::temp_directory_path() / "niminal-provider-auth-test";
  std::filesystem::remove_all(auth_root);
  std::filesystem::create_directories(auth_root / ".niminal");
  std::ofstream(auth_root / ".niminal" / "auth.json") << R"({"openai":{"key":"auth-openai-key"}})";
  const char* old_home = std::getenv("HOME");
  const char* old_openai_key = std::getenv("OPENAI_API_KEY");
  const std::string saved_home = old_home == nullptr ? "" : old_home;
  const std::string saved_openai_key = old_openai_key == nullptr ? "" : old_openai_key;
  const bool had_home = old_home != nullptr;
  const bool had_openai_key = old_openai_key != nullptr;
  setenv("HOME", auth_root.c_str(), 1);
  setenv("OPENAI_API_KEY", "environment-openai-key", 1);
  Config auth_cfg;
  bool auth_ok = false;
  std::string auth_error;
  if (auto result = select_provider(auth_cfg, "openai"); !result) {
    auth_error = result.error().what();
  } else {
    niminal::Agent auth_agent;
    apply_provider(auth_agent, auth_cfg);
    auth_ok = auth_agent.api_key == "auth-openai-key";
  }
  if (had_home) {
    setenv("HOME", saved_home.c_str(), 1);
  } else {
    unsetenv("HOME");
  }
  if (had_openai_key) {
    setenv("OPENAI_API_KEY", saved_openai_key.c_str(), 1);
  } else {
    unsetenv("OPENAI_API_KEY");
  }
  std::filesystem::remove_all(auth_root);
  if (!auth_error.empty()) {
    return fail(auth_error.c_str());
  }
  if (!auth_ok) {
    return fail("auth file should override environment key");
  }
  return 0;
}
