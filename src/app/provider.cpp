#include "provider.hpp"
#include "auth.hpp"
#include "thinking.hpp"

#include <niminal/text.hpp>

#include <algorithm>

namespace niminal::app {
void normalize_config(Config& cfg) {
  const niminal::ProviderSpec* spec = niminal::find_provider(cfg.provider);
  if (spec == nullptr) {
    return;
  }
  if (auto it = cfg.provider_api_urls.find(cfg.provider); it != cfg.provider_api_urls.end()) {
    cfg.api_url = it->second;
    return;
  }
  cfg.api_url = spec->endpoint;
}

void apply_provider(niminal::Agent& agent, const Config& cfg) {
  if (cfg.provider == "local") {
    const auto models = load_local_models();
    const auto selected = std::find_if(models.begin(), models.end(), [&](const LocalModel& model) {
      return model.name == cfg.model;
    });
    if (selected == models.end()) {
      throw niminal::Error("local model '" + cfg.model + "' is not in " +
                           local_models_path().string());
    }
    if (!supported_local_runtime(selected->runtime)) {
      throw niminal::Error("unsupported local runtime '" + selected->runtime + "'");
    }
    agent.provider = "local";
    agent.model_runtime = selected->runtime;
    agent.model = selected->model;
    agent.api_url = selected->api_url;
    agent.api_key = read_auth_key("local");
    agent.key_hint.clear();
    agent.extra_headers.clear();
    agent.session_routing = false;
    agent.stream_usage = false;
    agent.apply_cache = false;
    agent.prompt_cache_key = false;
    agent.extra = nlohmann::json::object();
    return;
  }
  const niminal::ProviderSpec* spec = niminal::find_provider(cfg.provider);
  if (spec == nullptr) {
    spec = niminal::find_provider("openrouter");
  }
  agent.provider = spec->name;
  agent.model_runtime.clear();
  agent.model = cfg.model.empty() ? spec->default_model : cfg.model;
  agent.api_url = cfg.api_url.empty() ? spec->endpoint : cfg.api_url;
  if (agent.api_url.empty()) {
    throw niminal::Error("missing API URL (set providers." + std::string(spec->name) +
                         ".api_url in ~/.niminal/config.json)");
  }
  agent.api_key = read_auth_key(spec->name);
  if (agent.api_key.empty()) {
    agent.api_key = niminal::read_api_key(*spec);
  }
  agent.key_hint = std::string(niminal::key_hint(*spec));
  agent.extra_headers = niminal::provider_headers(*spec);
  agent.session_routing = spec->session_routing;
  agent.stream_usage = spec->stream_usage;
  agent.apply_cache = spec->apply_cache;
  agent.prompt_cache_key = spec->prompt_cache_key;
  agent.extra = thinking_body(spec->name, agent.model, cfg.thinking);
}

void restore_config_from_session(Config& cfg, const Session& session, bool restore_provider,
                                 bool restore_model) {
  if (restore_provider) {
    if (auto p = session.last_provider(); !p.empty()) {
      if (const auto* spec = niminal::find_provider(p)) {
        cfg.provider = p;
        auto it = cfg.provider_api_urls.find(p);
        cfg.api_url = it != cfg.provider_api_urls.end() ? it->second : std::string(spec->endpoint);
      }
    }
  }
  if (restore_model) {
    if (auto model = session.last_model(); !model.empty()) {
      cfg.model = model;
    }
  }
  if (cfg.provider == "local") {
    const auto models = load_local_models();
    const auto selected = std::find_if(models.begin(), models.end(), [&](const LocalModel& model) {
      return model.name == cfg.model;
    });
    if (selected != models.end()) {
      cfg.api_url = selected->api_url;
    }
  }
}

niminal::Result<void> select_provider(Config& cfg, std::string_view name) {
  const auto n = niminal::lower_copy(std::string(name));
  if (n == "codex") {
    return std::unexpected(
        niminal::Error("codex is not wired (it talks to a local Codex app-server)"));
  }
  const niminal::ProviderSpec* spec = niminal::find_provider(n);
  if (spec == nullptr) {
    return std::unexpected(niminal::Error("unknown provider '" + std::string(name) + "' (use " +
                                          niminal::provider_names() + ")"));
  }
  if (n != "local" && spec->endpoint.empty() && !cfg.provider_api_urls.contains(n)) {
    return std::unexpected(
        niminal::Error("configure providers." + n + ".api_url in ~/.niminal/config.json first"));
  }
  if (!cfg.provider.empty() && !cfg.model.empty()) {
    cfg.last_models[cfg.provider] = cfg.model;
  }
  if (n == "local") {
    try {
      const auto models = load_local_models();
      if (models.empty()) {
        return std::unexpected(niminal::Error(local_models_path().string() + " has no models"));
      }
      const auto last = cfg.last_models.find("local");
      const auto selected =
          last == cfg.last_models.end()
              ? models.begin()
              : std::find_if(models.begin(), models.end(),
                             [&](const LocalModel& model) { return model.name == last->second; });
      const auto& model = selected == models.end() ? models.front() : *selected;
      if (!supported_local_runtime(model.runtime)) {
        return std::unexpected(niminal::Error("unsupported local runtime '" + model.runtime + "'"));
      }
      cfg.provider = "local";
      cfg.model = model.name;
      cfg.api_url = model.api_url;
      return {};
    } catch (const std::exception& e) {
      return std::unexpected(niminal::Error(e.what()));
    }
  }
  cfg.provider = spec->name;
  auto it = cfg.last_models.find(std::string(spec->name));
  cfg.model = it != cfg.last_models.end() && !it->second.empty() ? it->second : spec->default_model;
  auto url = cfg.provider_api_urls.find(std::string(spec->name));
  cfg.api_url = url != cfg.provider_api_urls.end() ? url->second : std::string(spec->endpoint);
  return {};
}

} // namespace niminal::app
