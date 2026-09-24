#include <niminal/providers.hpp>

#include <niminal/text.hpp>

#include <algorithm>
#include <cstdlib>
#include <vector>

namespace niminal {
namespace {

std::vector<ProviderSpec>& provider_registry() {
  static std::vector<ProviderSpec> providers = {
      {"anthropic",
       "https://api.anthropic.com/v1/messages",
       "claude-sonnet-4-6",
       {"ANTHROPIC_API_KEY"},
       "api.anthropic.com",
       false,
       false,
       true,
       false,
       {}},
      {"google",
       "https://generativelanguage.googleapis.com/v1beta",
       "gemini-3.5-flash-lite",
       {"GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_GENERATIVE_AI_API_KEY"},
       "generativelanguage.googleapis.com",
       false,
       false,
       false,
       false,
       {}},
      {"foundry",
       "",
       "gpt-5.4",
       {"AZURE_FOUNDRY_API_KEY"},
       "cognitiveservices.azure.com/openai/responses",
       false,
       false,
       false,
       false,
       {}},
      {"hyper",
       "https://hyper.charm.land/v1/chat/completions",
       "deepseek-v4-flash",
       {"HYPER_API_KEY"},
       "hyper.charm.land",
       false,
       false,
       false,
       false,
       {}},
      {"local", "", "", {}, "", false, false, false, false, {}},
      {"mistral",
       "https://api.mistral.ai/v1/chat/completions",
       "mistral-vibe-cli-with-tools",
       {"MISTRAL_API_KEY"},
       "api.mistral.ai",
       false,
       false,
       false,
       true,
       {}},
      {"openai",
       "https://api.openai.com/v1/chat/completions",
       "gpt-5",
       {"OPENAI_API_KEY"},
       "api.openai.com",
       false,
       true,
       false,
       true,
       {}},
      {"ollama",
       "https://ollama.com/v1/chat/completions",
       "gemma4:31b",
       {"OLLAMA_API_KEY"},
       "ollama.com",
       false,
       true,
       false,
       false,
       {}},
      {"opencode",
       "https://opencode.ai/zen/go/v1/chat/completions",
       "deepseek-v4.1-flash",
       {"OPENCODE_API_KEY"},
       "opencode.ai/zen/go",
       false,
       false,
       false,
       false,
       {}},
      {"opencodezen",
       "https://opencode.ai/zen/v1/chat/completions",
       "deepseek-v4-flash",
       {"OPENCODE_API_KEY"},
       "opencode.ai",
       false,
       false,
       true,
       false,
       {}},
      {"openrouter",
       "https://openrouter.ai/api/v1/chat/completions",
       "openai/gpt-4o-mini",
       {"OPENROUTER_API_KEY"},
       "openrouter.ai",
       true,
       true,
       true,
       true,
       {}},
  };
  return providers;
}

std::map<std::string, size_t>& provider_references() {
  static std::map<std::string, size_t> references;
  return references;
}

} // namespace

std::span<const ProviderSpec> all_providers() {
  return provider_registry();
}

const ProviderSpec* find_provider(std::string_view name) {
  const auto normalized = lower_copy(std::string(name));
  for (const auto& provider : provider_registry()) {
    if (normalized == provider.name) {
      return &provider;
    }
  }
  return nullptr;
}

bool register_provider(ProviderSpec provider) {
  if (provider.name.empty() || provider.endpoint.empty() || provider.default_model.empty() ||
      (provider.requires_api_key && provider.env_keys.empty())) {
    return false;
  }
  provider.name = lower_copy(provider.name);
  if (provider.models.empty()) {
    provider.models.push_back(provider.default_model);
  }
  auto& providers = provider_registry();
  const auto normalized = lower_copy(provider.name);
  const auto found = std::find_if(providers.begin(), providers.end(), [&](const auto& candidate) {
    return lower_copy(candidate.name) == normalized;
  });
  if (found != providers.end()) {
    if (*found != provider) {
      return false;
    }
    if (auto reference = provider_references().find(normalized);
        reference != provider_references().end()) {
      ++reference->second;
      return true;
    }
    return false;
  }
  providers.push_back(std::move(provider));
  provider_references()[normalized] = 1;
  return true;
}

void unregister_provider(std::string_view name) {
  auto& providers = provider_registry();
  const auto normalized = lower_copy(std::string(name));
  auto& references = provider_references();
  const auto reference = references.find(normalized);
  if (reference == references.end()) {
    return;
  }
  if (--reference->second != 0) {
    return;
  }
  std::erase_if(providers, [&](const auto& provider) { return provider.name == normalized; });
  references.erase(reference);
}

std::string provider_names() {
  std::string out;
  for (const auto& provider : provider_registry()) {
    if (!out.empty()) {
      out += '|';
    }
    out += provider.name;
  }
  return out;
}

std::string infer_provider(std::string_view api_url) {
  const std::string url(api_url);
  const ProviderSpec* best = nullptr;
  std::size_t best_len = 0;
  for (const auto& provider : provider_registry()) {
    if (provider.url_match.empty()) {
      continue;
    }
    if (url.find(provider.url_match) != std::string::npos && provider.url_match.size() > best_len) {
      best = &provider;
      best_len = provider.url_match.size();
    }
  }
  return best != nullptr ? std::string(best->name) : "openrouter";
}

std::string read_api_key(const ProviderSpec& provider) {
  for (const auto& key : provider.env_keys) {
    if (const char* value = std::getenv(key.c_str()); value != nullptr && *value != '\0') {
      return value;
    }
  }
  return {};
}

std::map<std::string, std::string> provider_headers(const ProviderSpec& provider) {
  if (provider.name == "anthropic") {
    return {{"anthropic-version", "2023-06-01"}};
  }
  if (provider.name == "openrouter") {
    return {{"HTTP-Referer", "https://niminal.dev"}, {"X-Title", "niminal"}};
  }
  return {};
}

} // namespace niminal
