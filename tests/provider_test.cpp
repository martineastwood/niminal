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
      (niminal::find_provider("ollama") == nullptr) ||
      (niminal::find_provider("GOOGLE") == nullptr) ||
      (niminal::find_provider("codex") != nullptr) ||
      (niminal::find_provider("gemini") != nullptr)) {
    return fail("find_provider");
  }
  auto names = niminal::provider_names();
  if (names.find("openrouter") == std::string::npos || names.find("mistral") == std::string::npos ||
      names.find("ollama") == std::string::npos) {
    return fail("provider_names");
  }
  if (niminal::infer_provider("https://api.anthropic.com/v1/messages") != "anthropic" ||
      niminal::infer_provider("https://generativelanguage.googleapis.com/v1beta") != "google" ||
      niminal::infer_provider("https://ollama.com/v1/chat/completions") != "ollama" ||
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
  Config ollama_cfg;
  if (auto result = select_provider(ollama_cfg, "ollama"); !result) {
    return fail(result.error().what());
  }
  if (ollama_cfg.provider != "ollama" || ollama_cfg.model != "gemma4:31b" ||
      ollama_cfg.api_url != "https://ollama.com/v1/chat/completions") {
    return fail("select ollama default model");
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
      !agent.extra_headers.empty() || !agent.apply_cache || agent.session_routing ||
      agent.stream_usage || agent.key_hint != "ANTHROPIC_API_KEY") {
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

  apply_provider(agent, ollama_cfg);
  if (agent.provider != "ollama" || agent.model != "gemma4:31b" ||
      agent.api_url != "https://ollama.com/v1/chat/completions" ||
      agent.key_hint != "OLLAMA_API_KEY" || agent.session_routing || !agent.stream_usage ||
      agent.apply_cache || agent.prompt_cache_key) {
    return fail("apply_provider ollama");
  }

  const char* old_ollama_key = std::getenv("OLLAMA_API_KEY");
  const std::string saved_ollama_key = old_ollama_key == nullptr ? "" : old_ollama_key;
  const bool had_ollama_key = old_ollama_key != nullptr;
  setenv("OLLAMA_API_KEY", "ollama-test-key", 1);
  const bool ollama_key_ok =
      niminal::read_api_key(*niminal::find_provider("ollama")) == "ollama-test-key";
  if (had_ollama_key) {
    setenv("OLLAMA_API_KEY", saved_ollama_key.c_str(), 1);
  } else {
    unsetenv("OLLAMA_API_KEY");
  }
  if (!ollama_key_ok) {
    return fail("read ollama API key");
  }

  Config custom;
  custom.provider = "openrouter";
  custom.api_url = "https://proxy.example.com/v1/chat/completions";
  custom.provider_api_urls["openrouter"] = custom.api_url;
  normalize_config(custom);
  apply_provider(agent, custom);
  if (custom.api_url != "https://proxy.example.com/v1/chat/completions" ||
      agent.api_url != custom.api_url) {
    return fail("custom api_url should persist across normalize_config");
  }

  const auto auth_root = std::filesystem::temp_directory_path() / "niminal-provider-auth-test";
  std::filesystem::remove_all(auth_root);
  std::filesystem::create_directories(auth_root / ".niminal");
  std::ofstream(auth_root / ".niminal" / "auth.json") << R"({"openai":{"key":"auth-openai-key"}})";
  std::ofstream(auth_root / ".niminal" / "models.json") << R"({"models":[
    {"provider":"local","name":"coding","runtime":"llamacpp","model":"local-coding","api_url":"http://localhost:8080/v1/chat/completions","context_window":32768},
    {"provider":"foundry","name":"coding","model":"deployment-a","api_url":"https://example.test/openai/responses?api-version=1"},
    {"provider":"foundry","name":"fast","model":"deployment-b","api_url":"https://other.test/openai/responses?api-version=2","context_window":64000}
  ]})";
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
  Config configured;
  if (auto result = select_provider(configured, "foundry"); !result) {
    return fail(result.error().what());
  }
  if (configured.model != "coding") {
    return fail("foundry should select first configured model");
  }
  apply_provider(agent, configured);
  if (agent.model != "deployment-a" ||
      agent.api_url != "https://example.test/openai/responses?api-version=1") {
    return fail("foundry model should resolve deployment and URL");
  }
  configured.model = "fast";
  apply_provider(agent, configured);
  if (agent.model != "deployment-b" ||
      agent.api_url != "https://other.test/openai/responses?api-version=2") {
    return fail("switching foundry model should switch URL");
  }
  configured.model = "missing";
  try {
    apply_provider(agent, configured);
    return fail("unknown foundry model should fail");
  } catch (const niminal::Error&) {
  }
  if (auto result = select_provider(configured, "local"); !result) {
    return fail(result.error().what());
  }
  apply_provider(agent, configured);
  if (configured.model != "coding" || agent.model != "local-coding" ||
      agent.api_url != "http://localhost:8080/v1/chat/completions") {
    return fail("local and foundry should allow the same configured name");
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
