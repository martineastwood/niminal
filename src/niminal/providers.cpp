#include <niminal/providers.hpp>

#include <niminal/text.hpp>

#include <array>
#include <cstdlib>

namespace niminal {
namespace {

constexpr std::array<std::string_view, 1> kAnthropicKeys = {"ANTHROPIC_API_KEY"};
constexpr std::array<std::string_view, 3> kGoogleKeys = {"GEMINI_API_KEY", "GOOGLE_API_KEY",
                                                         "GOOGLE_GENERATIVE_AI_API_KEY"};
constexpr std::array<std::string_view, 1> kHyperKeys = {"HYPER_API_KEY"};
constexpr std::array<std::string_view, 1> kMistralKeys = {"MISTRAL_API_KEY"};
constexpr std::array<std::string_view, 1> kOpenaiKeys = {"OPENAI_API_KEY"};
constexpr std::array<std::string_view, 1> kOpencodeKeys = {"OPENCODE_API_KEY"};
constexpr std::array<std::string_view, 1> kOpenrouterKeys = {"OPENROUTER_API_KEY"};

constexpr ProviderSpec kProviders[] = {
    {"anthropic", "https://api.anthropic.com/v1/messages", "claude-sonnet-4-6", kAnthropicKeys,
     "api.anthropic.com", false, false, true, false},
    {"google", "https://generativelanguage.googleapis.com/v1beta", "gemini-3.5-flash-lite",
     kGoogleKeys, "generativelanguage.googleapis.com", false, false, false, false},
    {"hyper", "https://hyper.charm.land/v1/chat/completions", "deepseek-v4-flash", kHyperKeys,
     "hyper.charm.land", false, false, false, false},
    {"mistral", "https://api.mistral.ai/v1/chat/completions", "mistral-vibe-cli-with-tools",
     kMistralKeys, "api.mistral.ai", false, false, false, true},
    {"openai", "https://api.openai.com/v1/chat/completions", "gpt-5", kOpenaiKeys, "api.openai.com",
     false, true, false, true},
    {"opencode", "https://opencode.ai/zen/go/v1/chat/completions", "deepseek-v4.1-flash",
     kOpencodeKeys, "opencode.ai/zen/go", false, false, false, false},
    {"opencodezen", "https://opencode.ai/zen/v1/chat/completions", "deepseek-v4-flash",
     kOpencodeKeys, "opencode.ai", false, false, true, false},
    {"openrouter", "https://openrouter.ai/api/v1/chat/completions", "openai/gpt-4o-mini",
     kOpenrouterKeys, "openrouter.ai", true, true, true, true},
};

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
  const ProviderSpec* best = nullptr;
  std::size_t best_len = 0;
  for (const auto& provider : kProviders) {
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
  for (const auto key : provider.env_keys) {
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
