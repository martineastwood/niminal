#include <niminal/providers.hpp>

#include <niminal/text.hpp>

#include <cstdlib>
#include <vector>

namespace niminal {
namespace {

constexpr ProviderSpec kProviders[] = {
    {"anthropic", "https://api.anthropic.com/v1/messages", "claude-sonnet-4-6", "ANTHROPIC_API_KEY",
     false, false, true, false},
    {"google", "https://generativelanguage.googleapis.com/v1beta", "gemini-3.5-flash-lite",
     "GEMINI_API_KEY", false, false, false, false},
    {"hyper", "https://hyper.charm.land/v1/chat/completions", "deepseek-v4-flash", "HYPER_API_KEY",
     false, false, false, false},
    {"mistral", "https://api.mistral.ai/v1/chat/completions", "mistral-vibe-cli-with-tools",
     "MISTRAL_API_KEY", false, false, false, true},
    {"openai", "https://api.openai.com/v1/chat/completions", "gpt-5", "OPENAI_API_KEY", false, true,
     false, true},
    {"opencode", "https://opencode.ai/zen/go/v1/chat/completions", "deepseek-v4.1-flash",
     "OPENCODE_API_KEY", false, false, false, false},
    {"opencodezen", "https://opencode.ai/zen/v1/chat/completions", "deepseek-v4-flash",
     "OPENCODE_API_KEY", false, false, true, false},
    {"openrouter", "https://openrouter.ai/api/v1/chat/completions", "openai/gpt-4o-mini",
     "OPENROUTER_API_KEY", true, true, true, true},
};

std::vector<std::string_view> env_keys(std::string_view name) {
  if (name == "anthropic") {
    return {"ANTHROPIC_API_KEY"};
  }
  if (name == "google") {
    return {"GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_GENERATIVE_AI_API_KEY"};
  }
  if (name == "hyper") {
    return {"HYPER_API_KEY"};
  }
  if (name == "mistral") {
    return {"MISTRAL_API_KEY"};
  }
  if (name == "openai") {
    return {"OPENAI_API_KEY"};
  }
  if (name == "opencode" || name == "opencodezen") {
    return {"OPENCODE_API_KEY"};
  }
  return {"OPENROUTER_API_KEY"};
}

} // namespace

std::span<const ProviderSpec> all_providers() {
  return kProviders;
}

const ProviderSpec* find_provider(std::string_view name) {
  const auto normalized = lower_copy(std::string(name));
  for (const auto& provider : kProviders) {
    if (normalized == provider.name) {
      return &provider;
    }
  }
  return nullptr;
}

std::string provider_names() {
  std::string out;
  for (const auto& provider : kProviders) {
    if (!out.empty()) {
      out += '|';
    }
    out += provider.name;
  }
  return out;
}

std::string infer_provider(std::string_view api_url) {
  const std::string url(api_url);
  if (url.find("api.anthropic.com") != std::string::npos) {
    return "anthropic";
  }
  if (url.find("generativelanguage.googleapis.com") != std::string::npos) {
    return "google";
  }
  if (url.find("hyper.charm.land") != std::string::npos) {
    return "hyper";
  }
  if (url.find("api.mistral.ai") != std::string::npos) {
    return "mistral";
  }
  if (url.find("api.openai.com") != std::string::npos) {
    return "openai";
  }
  if (url.find("opencode.ai/zen/go") != std::string::npos) {
    return "opencode";
  }
  if (url.find("opencode.ai") != std::string::npos) {
    return "opencodezen";
  }
  return "openrouter";
}

std::string read_api_key(const ProviderSpec& provider) {
  for (const auto key : env_keys(provider.name)) {
    if (const char* value = std::getenv(key.data()); value != nullptr && *value != '\0') {
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
