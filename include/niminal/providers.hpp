#pragma once

#include <map>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace niminal {

struct ProviderSpec {
  std::string name;
  std::string endpoint;
  std::string default_model;
  std::vector<std::string> env_keys;
  std::string url_match;
  bool session_routing = false;
  bool stream_usage = false;
  bool apply_cache = false;
  bool prompt_cache_key = false;
  std::vector<std::string> models;
  bool requires_api_key = true;
  bool operator==(const ProviderSpec&) const = default;
};

inline std::string_view key_hint(const ProviderSpec& provider) {
  return provider.env_keys.empty() ? std::string_view{}
                                   : std::string_view(provider.env_keys.front());
}

std::span<const ProviderSpec> all_providers();
const ProviderSpec* find_provider(std::string_view name);
bool register_provider(ProviderSpec provider);
void unregister_provider(std::string_view name);
std::string provider_names();
std::string infer_provider(std::string_view api_url);
std::string read_api_key(const ProviderSpec& provider);
std::map<std::string, std::string> provider_headers(const ProviderSpec& provider);

} // namespace niminal
