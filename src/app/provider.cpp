#include "provider.hpp"
#include "auth.hpp"
#include "models_dev.hpp"
#include "thinking.hpp"

#include <niminal/text.hpp>

#include <cail/anthropic.hpp>
#include <cail/chat_completions.hpp>
#include <cail/foundry.hpp>
#include <cail/gemini.hpp>
#include <cail/hyper.hpp>
#include <cail/local.hpp>
#include <cail/mistral.hpp>
#include <cail/ollama_cloud.hpp>
#include <cail/openai.hpp>
#include <cail/opencode.hpp>
#include <cail/openrouter.hpp>

#include <algorithm>
#include <cstdlib>
#include <map>
#include <optional>

namespace niminal::app {
namespace {

struct ProviderChoice {
  std::string_view name;
  std::string_view model;
};
constexpr ProviderChoice builtins[] = {
    {"anthropic", "claude-sonnet-4-6"},
    {"google", "gemini-3.5-flash-lite"},
    {"foundry", ""},
    {"hyper", "deepseek-v4-flash"},
    {"local", ""},
    {"mistral", "mistral-vibe-cli-with-tools"},
    {"openai", "gpt-5"},
    {"ollama", "gemma4:31b"},
    {"opencode", "deepseek-v4.1-flash"},
    {"opencodezen", "deepseek-v4-flash"},
    {"openrouter", "openai/gpt-4o-mini"},
};

struct Registration {
  CustomProvider spec;
  size_t references = 1;
};
std::map<std::string, Registration> custom_providers;

std::string read_environment(const std::vector<std::string>& names) {
  for (const auto& name : names) {
    if (const auto* value = std::getenv(name.c_str()); value != nullptr && *value != '\0') {
      return value;
    }
  }
  return {};
}

std::optional<cail::OpenCodeApiFamily> opencode_family(std::string_view sdk) {
  if (sdk == "@ai-sdk/openai-compatible") {
    return cail::OpenCodeApiFamily::chat_completions;
  }
  if (sdk == "@ai-sdk/openai") {
    return cail::OpenCodeApiFamily::responses;
  }
  if (sdk == "@ai-sdk/anthropic") {
    return cail::OpenCodeApiFamily::anthropic_messages;
  }
  if (sdk == "@ai-sdk/google") {
    return cail::OpenCodeApiFamily::gemini;
  }
  return std::nullopt;
}

cail::LanguageModel make_model(const Config& config, const std::string& model, std::string key) {
  if (model.empty()) {
    throw Error("missing model");
  }
  if (config.provider == "foundry") {
    if (config.api_url.empty()) {
      throw Error("missing Foundry API URL (configure the model in ~/.niminal/models.json)");
    }
    return cail::create_foundry({.api_key = key})(cail::FoundryDeployment{
        .endpoint = config.api_url,
        .deployment = model,
    });
  }
  if (config.provider == "anthropic") {
    return cail::create_anthropic(cail::AnthropicSettings{
        .api_key = key,
        .base_url = config.api_url.empty() ? cail::AnthropicSettings{}.base_url : config.api_url,
        .max_tokens = 16384,
    })(model);
  }
  if (config.provider == "google") {
    if (key.empty()) {
      key = read_environment({"GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_GENERATIVE_AI_API_KEY"});
    }
    auto google_model = model;
    if (google_model.starts_with("models/")) {
      google_model.erase(0, 7);
    }
    return cail::create_gemini(cail::GeminiSettings{
        .api_key = key,
        .base_url = config.api_url.empty() ? cail::GeminiSettings{}.base_url : config.api_url,
    })(google_model);
  }
  if (config.provider == "opencode" || config.provider == "opencodezen") {
    const auto sdk = lookup_model_sdk(config.provider, model);
    const auto family = opencode_family(sdk);
    if (!family) {
      throw Error("Missing or unsupported API family for " + model +
                  " in the cached models.dev catalog. Run /models refresh.");
    }
    return cail::create_opencode(cail::OpenCodeSettings{
        .api_key = key,
        .service =
            config.provider == "opencode" ? cail::OpenCodeService::go : cail::OpenCodeService::zen,
        .base_url = config.api_url,
        .anthropic_max_tokens = 16384,
    })(model, *family);
  }
  if (config.provider == "hyper") {
    return cail::create_hyper(cail::HyperSettings{
        .api_key = key,
        .endpoint = config.api_url.empty() ? cail::HyperSettings{}.endpoint : config.api_url,
    })(model);
  }
  if (config.provider == "ollama") {
    return cail::create_ollama_cloud(cail::OllamaCloudSettings{
        .api_key = key,
        .endpoint = config.api_url.empty() ? cail::OllamaCloudSettings{}.endpoint : config.api_url,
    })(model);
  }
  if (config.provider == "local") {
    return cail::create_local(cail::LocalSettings{
        .api_key = key,
        .endpoint = config.api_url.empty() ? cail::LocalSettings{}.endpoint : config.api_url,
    })(model);
  }
  if (config.provider == "openai") {
    return cail::create_openai(cail::OpenAIProviderSettings{
        .api_key = key,
        .base_url =
            config.api_url.empty() ? cail::OpenAIProviderSettings{}.base_url : config.api_url,
    })(model);
  }
  if (config.provider == "mistral") {
    return cail::create_mistral(cail::MistralSettings{
        .api_key = key,
        .endpoint = config.api_url.empty() ? cail::MistralSettings{}.endpoint : config.api_url,
    })(model);
  }
  if (config.provider == "openrouter") {
    return cail::create_openrouter(cail::OpenRouterSettings{
        .api_key = key,
        .headers = {{"HTTP-Referer", "https://niminal.dev"}, {"X-Title", "niminal"}},
        .endpoint = config.api_url.empty() ? cail::OpenRouterSettings{}.endpoint : config.api_url,
    })(model);
  }
  const auto& custom = custom_providers.at(config.provider).spec;
  return cail::create_chat_completions({
      .endpoint = config.api_url.empty() ? custom.endpoint : config.api_url,
      .api_key = key.empty() ? read_environment(custom.env_keys) : key,
      .prompt_cache_key = custom.prompt_cache_key,
      .session_body = custom.session_routing,
  })(model);
}

} // namespace

std::string provider_default_model(std::string_view name) {
  for (const auto& provider : builtins) {
    if (provider.name == name) {
      return std::string(provider.model);
    }
  }
  return custom_providers.at(std::string(name)).spec.default_model;
}

bool has_provider(std::string_view name) {
  const auto normalized = lower_copy(std::string(name));
  return std::ranges::any_of(builtins,
                             [&](const auto& provider) { return provider.name == normalized; }) ||
         custom_providers.contains(normalized);
}

std::vector<std::string> provider_names() {
  std::vector<std::string> names;
  for (const auto& provider : builtins) {
    names.emplace_back(provider.name);
  }
  for (const auto& [name, registration] : custom_providers) {
    names.push_back(name);
  }
  return names;
}

std::vector<std::string> provider_models(std::string_view name) {
  const auto found = custom_providers.find(lower_copy(std::string(name)));
  return found == custom_providers.end() ? std::vector<std::string>{} : found->second.spec.models;
}

bool register_provider(CustomProvider provider) {
  if (provider.name.empty() || provider.endpoint.empty() || provider.default_model.empty() ||
      (provider.requires_api_key && provider.env_keys.empty())) {
    return false;
  }
  provider.name = lower_copy(provider.name);
  if (provider.models.empty()) {
    provider.models.push_back(provider.default_model);
  }
  if (auto found = custom_providers.find(provider.name); found != custom_providers.end()) {
    if (found->second.spec != provider) {
      return false;
    }
    ++found->second.references;
    return true;
  }
  if (has_provider(provider.name)) {
    return false;
  }
  const auto name = provider.name;
  custom_providers.emplace(name, Registration{std::move(provider)});
  return true;
}

void unregister_provider(std::string_view name) {
  const auto found = custom_providers.find(lower_copy(std::string(name)));
  if (found != custom_providers.end() && --found->second.references == 0) {
    custom_providers.erase(found);
  }
}

void normalize_config(Config& cfg) {
  const auto url = cfg.provider_api_urls.find(cfg.provider);
  cfg.api_url = url == cfg.provider_api_urls.end() ? "" : url->second;
}

void apply_provider(niminal::Agent& agent, Config& cfg, std::string_view api_key) {
  cfg.provider = lower_copy(cfg.provider);
  if (!has_provider(cfg.provider)) {
    throw Error("unknown provider '" + cfg.provider + "'");
  }
  std::string model = cfg.model.empty() ? provider_default_model(cfg.provider) : cfg.model;
  cfg.model_runtime.clear();
  if (cfg.provider == "local" || cfg.provider == "foundry") {
    const auto models = models_for(cfg.provider);
    const auto selected = std::ranges::find(models, cfg.model, &ConfiguredModel::name);
    if (selected == models.end()) {
      throw Error(cfg.provider + " model '" + cfg.model + "' is not in " + models_path().string());
    }
    model = selected->model;
    cfg.api_url = selected->api_url;
    cfg.model_runtime = selected->runtime;
  }
  agent.language_model =
      make_model(cfg, model, api_key.empty() ? read_auth_key(cfg.provider) : std::string(api_key));
  agent.model = model;
  agent.stream_usage.reset();
  agent.apply_cache =
      cfg.provider == "anthropic" || cfg.provider == "opencodezen" || cfg.provider == "openrouter";
  if (const auto custom = custom_providers.find(cfg.provider); custom != custom_providers.end()) {
    agent.stream_usage = custom->second.spec.stream_usage;
    agent.apply_cache = custom->second.spec.apply_cache;
  }
  agent.extra = thinking_body(cfg.provider, model, cfg.thinking);
}

void restore_config_from_session(Config& cfg, const Session& session, bool restore_provider,
                                 bool restore_model) {
  if (restore_provider) {
    if (auto p = session.last_provider(); !p.empty()) {
      if (has_provider(p)) {
        cfg.provider = p;
        normalize_config(cfg);
      }
    }
  }
  if (restore_model) {
    if (auto model = session.last_model(); !model.empty()) {
      cfg.model = model;
    }
  }
  if (cfg.provider == "local" || cfg.provider == "foundry") {
    const auto models = models_for(cfg.provider);
    const auto selected =
        std::find_if(models.begin(), models.end(),
                     [&](const ConfiguredModel& model) { return model.name == cfg.model; });
    if (selected != models.end()) {
      cfg.api_url = selected->api_url;
    }
  }
}

niminal::Result<void> select_provider(Config& cfg, std::string_view name) {
  const auto n = niminal::lower_copy(std::string(name));
  if (!has_provider(n)) {
    return std::unexpected(
        Error("unknown provider '" + std::string(name) + "' (use /provider to list providers)"));
  }
  if (!cfg.provider.empty() && !cfg.model.empty()) {
    cfg.last_models[cfg.provider] = cfg.model;
  }
  if (n == "local" || n == "foundry") {
    try {
      const auto models = models_for(n);
      if (models.empty()) {
        return std::unexpected(niminal::Error(models_path().string() + " has no " + n + " models"));
      }
      const auto last = cfg.last_models.find(n);
      const auto selected =
          last == cfg.last_models.end()
              ? models.begin()
              : std::find_if(models.begin(), models.end(), [&](const ConfiguredModel& model) {
                  return model.name == last->second;
                });
      const auto& model = selected == models.end() ? models.front() : *selected;
      cfg.provider = n;
      cfg.model = model.name;
      cfg.api_url = model.api_url;
      return {};
    } catch (const std::exception& e) {
      return std::unexpected(niminal::Error(e.what()));
    }
  }
  cfg.provider = n;
  auto it = cfg.last_models.find(n);
  cfg.model =
      it != cfg.last_models.end() && !it->second.empty() ? it->second : provider_default_model(n);
  normalize_config(cfg);
  return {};
}

} // namespace niminal::app
