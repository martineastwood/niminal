#include "provider.hpp"
#include "thinking.hpp"

#include <niminal/text.hpp>

namespace niminal::app {
void normalize_config(Config& cfg) {
  const niminal::ProviderSpec* spec = niminal::find_provider(cfg.provider);
  if ((spec != nullptr) && cfg.api_url != spec->endpoint) {
    cfg.api_url = spec->endpoint;
  }
}

void apply_provider(niminal::Agent& agent, const Config& cfg) {
  const niminal::ProviderSpec* spec = niminal::find_provider(cfg.provider);
  if (spec == nullptr) {
    spec = niminal::find_provider("openrouter");
  }
  agent.provider = spec->name;
  agent.model = cfg.model.empty() ? spec->default_model : cfg.model;
  agent.api_url = cfg.api_url.empty() ? spec->endpoint : cfg.api_url;
  agent.api_key = niminal::read_api_key(*spec);
  agent.key_hint = spec->key_hint;
  agent.extra_headers = niminal::provider_headers(*spec);
  agent.session_routing = spec->session_routing;
  agent.stream_usage = spec->stream_usage;
  agent.apply_cache = spec->apply_cache;
  agent.prompt_cache_key = spec->prompt_cache_key;
  agent.extra = thinking_body(spec->name, agent.model, cfg.thinking);
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
  if (!cfg.provider.empty() && !cfg.model.empty()) {
    cfg.last_models[cfg.provider] = cfg.model;
  }
  cfg.provider = spec->name;
  auto it = cfg.last_models.find(std::string(spec->name));
  cfg.model = it != cfg.last_models.end() && !it->second.empty() ? it->second : spec->default_model;
  cfg.api_url = spec->endpoint;
  return {};
}

} // namespace niminal::app
