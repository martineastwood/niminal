#include "provider.hpp"
#include "thinking.hpp"

#include <cstdlib>
#include <map>
#include <sstream>

namespace niminal::app {
namespace {

constexpr ProviderSpec kProviders[] = {
    {"anthropic", "https://api.anthropic.com/v1/messages", "claude-sonnet-4-6",
     false, false, true, false},
    {"google", "https://generativelanguage.googleapis.com/v1beta",
     "gemini-3.5-flash-lite", false, false, false, false},
    {"hyper", "https://hyper.charm.land/v1/chat/completions",
     "deepseek-v4-flash", false, false, false, false},
    {"mistral", "https://api.mistral.ai/v1/chat/completions",
     "mistral-vibe-cli-with-tools", false, false, false, true},
    {"openai", "https://api.openai.com/v1/chat/completions", "gpt-5", false,
     true, false, true},
    {"opencode", "https://opencode.ai/zen/go/v1/chat/completions",
     "deepseek-v4.1-flash", false, false, false, false},
    {"opencodezen", "https://opencode.ai/zen/v1/chat/completions",
     "deepseek-v4-flash", false, false, true, false},
    {"openrouter", "https://openrouter.ai/api/v1/chat/completions",
     "openai/gpt-4o-mini", true, true, true, true},
};

std::string lower(std::string s) {
  for (char& c : s)
    if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
  return s;
}

std::vector<const char*> env_keys(std::string_view name) {
  if (name == "anthropic") return {"ANTHROPIC_API_KEY"};
  if (name == "google")
    return {"GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_GENERATIVE_AI_API_KEY"};
  if (name == "hyper") return {"HYPER_API_KEY"};
  if (name == "mistral") return {"MISTRAL_API_KEY"};
  if (name == "openai") return {"OPENAI_API_KEY"};
  if (name == "opencode" || name == "opencodezen") return {"OPENCODE_API_KEY"};
  return {"OPENROUTER_API_KEY"};
}

std::map<std::string, std::string> extra_headers(std::string_view name) {
  if (name == "anthropic") return {{"anthropic-version", "2023-06-01"}};
  if (name == "openrouter")
    return {{"HTTP-Referer", "https://niminal.dev"}, {"X-Title", "niminal"}};
  return {};
}

}  // namespace

const ProviderSpec* find_provider(std::string_view name) {
  auto n = lower(std::string(name));
  for (const auto& spec : kProviders)
    if (n == spec.name) return &spec;
  return nullptr;
}

std::vector<const ProviderSpec*> all_providers() {
  std::vector<const ProviderSpec*> out;
  for (const auto& spec : kProviders) out.push_back(&spec);
  return out;
}

std::string provider_names() {
  std::ostringstream out;
  bool first = true;
  for (const auto& spec : kProviders) {
    if (!first) out << '|';
    first = false;
    out << spec.name;
  }
  return out.str();
}

std::string infer_provider(std::string_view api_url) {
  std::string url(api_url);
  if (url.find("api.anthropic.com") != std::string::npos) return "anthropic";
  if (url.find("generativelanguage.googleapis.com") != std::string::npos)
    return "google";
  if (url.find("hyper.charm.land") != std::string::npos) return "hyper";
  if (url.find("api.mistral.ai") != std::string::npos) return "mistral";
  if (url.find("api.openai.com") != std::string::npos) return "openai";
  if (url.find("opencode.ai/zen/go") != std::string::npos) return "opencode";
  if (url.find("opencode.ai") != std::string::npos) return "opencodezen";
  return "openrouter";
}

std::string read_api_key(const ProviderSpec& spec) {
  for (auto key : env_keys(spec.name)) {
    if (const char* v = std::getenv(key); v && *v) return v;
  }
  return {};
}

void apply_provider(niminal::Agent& agent, const Config& cfg) {
  const ProviderSpec* spec = find_provider(cfg.provider);
  if (!spec) spec = find_provider("openrouter");
  agent.provider = spec->name;
  agent.model = cfg.model.empty() ? spec->default_model : cfg.model;
  bool stale_compat =
      (spec->name == std::string("anthropic") &&
       cfg.api_url.find("/chat/completions") != std::string::npos) ||
      (spec->name == std::string("google") &&
       cfg.api_url.find("/openai/") != std::string::npos);
  agent.api_url = (cfg.api_url.empty() || stale_compat) ? spec->endpoint
                                                       : cfg.api_url;
  agent.api_key = read_api_key(*spec);
  agent.key_hint = env_keys(spec->name).front();
  agent.extra_headers = extra_headers(spec->name);
  agent.session_routing = spec->session_routing;
  agent.stream_usage = spec->stream_usage;
  agent.apply_cache = spec->apply_cache;
  agent.prompt_cache_key = spec->prompt_cache_key;
  agent.extra = thinking_body(spec->name, agent.model, cfg.thinking);
}

bool select_provider(Config& cfg, std::string_view name, std::string* err) {
  auto n = lower(std::string(name));
  if (n == "codex") {
    if (err)
      *err = "codex is not wired (it talks to a local Codex app-server)";
    return false;
  }
  const ProviderSpec* spec = find_provider(n);
  if (!spec) {
    if (err)
      *err = "unknown provider '" + std::string(name) + "' (use " +
             provider_names() + ")";
    return false;
  }
  if (!cfg.provider.empty() && !cfg.model.empty())
    cfg.last_models[cfg.provider] = cfg.model;
  cfg.provider = spec->name;
  auto it = cfg.last_models.find(spec->name);
  cfg.model = it != cfg.last_models.end() && !it->second.empty()
                  ? it->second
                  : spec->default_model;
  cfg.api_url = spec->endpoint;
  return true;
}

}  // namespace niminal::app
