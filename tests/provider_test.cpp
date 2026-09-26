#include "models_dev.hpp"
#include "provider.hpp"

#include <cail/http.hpp>
#include <niminal/chat.hpp>

#include <algorithm>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>

using niminal::app::apply_provider;
using niminal::app::Config;
using niminal::app::normalize_config;
using niminal::app::select_provider;
using niminal::app::set_catalog_cache_path;

static int fail(const char* msg) {
  std::cerr << msg << '\n';
  return 1;
}

static cail::HttpRequest model_request(const niminal::Agent& agent) {
  cail::HttpRequest captured;
  cail::GenerationRequest request;
  request.messages = {{.role = cail::MessageRole::user,
                       .content = {cail::TextPart{.text = "hello", .provider_options = {}}},
                       .tool_call_id = {},
                       .tool_calls = {},
                       .provider_options = {}}};
  request.session_id = "test-session";
  request.before_request = [&](cail::HttpRequest& http) {
    captured = http;
    throw niminal::Error("request captured");
  };
  try {
    const auto response = agent.language_model.generate(request);
    if (!response) {
      throw niminal::Error(response.error().message);
    }
  } catch (const niminal::Error&) {
    if (captured.url.empty()) {
      throw;
    }
  }
  return captured;
}

int main() {
  if ((!niminal::app::has_provider("anthropic")) || (!niminal::app::has_provider("ollama")) ||
      (!niminal::app::has_provider("GOOGLE")) || (niminal::app::has_provider("codex")) ||
      (niminal::app::has_provider("gemini"))) {
    return fail("find_provider");
  }
  auto names = niminal::app::provider_names();
  if (std::ranges::find(names, "openrouter") == names.end() ||
      std::ranges::find(names, "mistral") == names.end() ||
      std::ranges::find(names, "ollama") == names.end()) {
    return fail("provider_names");
  }

  const auto catalog_root =
      std::filesystem::temp_directory_path() / "niminal-provider-models-dev-test";
  std::filesystem::remove_all(catalog_root);
  set_catalog_cache_path(catalog_root / "models-dev.json");
  Config opencode_cfg;
  opencode_cfg.provider = "opencode";
  opencode_cfg.model = "deepseek-v4.1-flash";
  niminal::Agent opencode_agent;
  apply_provider(opencode_agent, opencode_cfg, "test");
  opencode_agent.conversation_id = "opencode-session";
  if (model_request(opencode_agent).url != "https://opencode.ai/zen/go/v1/chat/completions") {
    return fail("OpenCode should start without a model catalog");
  }
  std::filesystem::remove_all(catalog_root);

  // Built-in routing comes from Cail presets; extension routing remains configurable.
  for (const auto* name : {"openrouter", "openai", "mistral", "test-routing"}) {
    const bool custom = std::string_view(name) == "test-routing";
    if (custom && !niminal::app::register_provider({.name = name,
                                                    .endpoint = "http://127.0.0.1:1",
                                                    .default_model = "test",
                                                    .session_routing = true,
                                                    .prompt_cache_key = true,
                                                    .requires_api_key = false})) {
      return fail("register routing provider");
    }
    Config routing;
    routing.provider = name;
    routing.api_url = "http://127.0.0.1:1";
    niminal::Agent routed;
    apply_provider(routed, routing, "test");
    routed.conversation_id = "routing-session";
    niminal::ChatRequest request;
    routed.fill_chat(request);
    request.messages = niminal::json::array({{{"role", "user"}, {"content", "hello"}}});
    niminal::json body;
    request.before_provider_request = [&](niminal::json& payload) {
      body = payload;
      throw niminal::Error("request captured");
    };
    try {
      niminal::stream_chat(request);
    } catch (const niminal::Error& error) {
      if (body.is_null()) {
        std::cerr << name << ": " << error.what() << '\n';
        return fail("routing request was not captured");
      }
    }
    if (custom) {
      niminal::app::unregister_provider(name);
    }
    if (body.value("prompt_cache_key", "") != "routing-session" ||
        ((custom || std::string_view(name) == "openrouter") &&
         body.value("session_id", "") != "routing-session")) {
      return fail("provider session routing");
    }
  }

  for (const auto& [name, endpoint] : std::vector<std::pair<std::string, std::string>>{
           {"anthropic", "https://api.anthropic.com/v1/messages"},
           {"google", "https://generativelanguage.googleapis.com/v1beta/models/"
                      "gemini-3.5-flash-lite:generateContent"},
           {"openai", "https://api.openai.com/v1/responses"},
           {"openrouter", "https://openrouter.ai/api/v1/chat/completions"},
           {"hyper", "https://hyper.charm.land/v1/chat/completions"},
           {"mistral", "https://api.mistral.ai/v1/chat/completions"},
           {"ollama", "https://ollama.com/v1/chat/completions"}}) {
    Config defaults;
    if (!select_provider(defaults, name)) {
      return fail("select built-in provider");
    }
    niminal::Agent model;
    apply_provider(model, defaults, "test");
    if (model.stream_usage.has_value()) {
      return fail("built-in providers should use Cail streaming defaults");
    }
    const auto http = model_request(model);
    if (!defaults.api_url.empty() || http.url != endpoint) {
      return fail("Cail should supply built-in endpoints without Niminal storing them");
    }
    if (name == "openrouter" && !std::ranges::any_of(http.headers, [](const auto& header) {
          return header.name == "X-Title" && header.value == "niminal";
        })) {
      return fail("OpenRouter attribution belongs in Cail settings");
    }
  }

  niminal::app::CustomProvider extension{.name = "extension-test",
                                         .endpoint = "https://proxy.example/v1/chat/completions",
                                         .default_model = "custom-model",
                                         .requires_api_key = false};
  if (!niminal::app::register_provider(extension) || !niminal::app::register_provider(extension)) {
    return fail("overlapping extension runtimes can register the same provider");
  }
  auto conflict = extension;
  conflict.endpoint = "https://different.example/v1/chat/completions";
  if (niminal::app::register_provider(conflict)) {
    return fail("conflicting extension provider must be rejected");
  }
  Config extension_cfg;
  if (!select_provider(extension_cfg, extension.name)) {
    return fail("select custom provider");
  }
  niminal::Agent extension_agent;
  apply_provider(extension_agent, extension_cfg);
  if (!extension_agent.stream_usage.has_value() || *extension_agent.stream_usage ||
      model_request(extension_agent).url != extension.endpoint ||
      niminal::app::provider_models(extension.name) != std::vector<std::string>{"custom-model"}) {
    return fail("custom providers retain their endpoint and model list");
  }
  niminal::app::unregister_provider(extension.name);
  if (!niminal::app::has_provider(extension.name)) {
    return fail("provider remains until its final runtime unloads");
  }
  niminal::app::unregister_provider(extension.name);
  niminal::app::unregister_provider("openai");
  if (niminal::app::has_provider(extension.name) || !niminal::app::has_provider("openai")) {
    return fail("unloading extensions must not affect built-in providers");
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
      !ollama_cfg.api_url.empty()) {
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
  if (cfg.provider != "anthropic" ||
      model_request(agent).url != "https://api.anthropic.com/v1/messages" || !agent.apply_cache ||
      agent.stream_usage.has_value() || !agent.language_model) {
    return fail("apply_provider anthropic");
  }

  Config unknown;
  unknown.provider = "missing";
  try {
    apply_provider(agent, unknown);
    return fail("unknown provider should not silently select another model");
  } catch (const niminal::Error&) {
  }

  Config stale;
  stale.provider = "anthropic";
  stale.model = "claude-haiku-4-5";
  stale.api_url = "https://api.anthropic.com/v1/chat/completions";
  normalize_config(stale);
  apply_provider(agent, stale);
  if (!stale.api_url.empty() ||
      model_request(agent).url != "https://api.anthropic.com/v1/messages") {
    return fail("stale api_url should normalize to provider endpoint");
  }

  apply_provider(agent, ollama_cfg);
  if (ollama_cfg.provider != "ollama" || agent.model != "gemma4:31b" ||
      model_request(agent).url != "https://ollama.com/v1/chat/completions" ||
      agent.stream_usage.has_value() || agent.apply_cache || !agent.language_model) {
    return fail("apply_provider ollama");
  }

  const char* old_ollama_key = std::getenv("OLLAMA_API_KEY");
  const std::string saved_ollama_key = old_ollama_key == nullptr ? "" : old_ollama_key;
  const bool had_ollama_key = old_ollama_key != nullptr;
  setenv("OLLAMA_API_KEY", "ollama-test-key", 1);
  apply_provider(agent, ollama_cfg);
  const auto ollama_request = model_request(agent);
  const bool ollama_key_ok = std::ranges::any_of(ollama_request.headers, [](const auto& header) {
    return header.name == "Authorization" && header.value == "Bearer ollama-test-key";
  });
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
  if (custom.api_url != "https://proxy.example.com/v1/chat/completions") {
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
    const auto has_key = [](const cail::HttpRequest& request, std::string_view key) {
      return std::any_of(request.headers.begin(), request.headers.end(), [&](const auto& header) {
        return header.name == "Authorization" && header.value == key;
      });
    };
    auth_ok = has_key(model_request(auth_agent), "Bearer auth-openai-key");
    apply_provider(auth_agent, auth_cfg, "override-key");
    auth_ok = auth_ok && has_key(model_request(auth_agent), "Bearer override-key");
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
      model_request(agent).url != "https://example.test/openai/responses?api-version=1") {
    return fail("foundry model should resolve deployment and URL");
  }
  configured.model = "fast";
  apply_provider(agent, configured);
  if (agent.model != "deployment-b" ||
      model_request(agent).url != "https://other.test/openai/responses?api-version=2") {
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
      model_request(agent).url != "http://localhost:8080/v1/chat/completions" ||
      configured.model_runtime != "llamacpp") {
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
