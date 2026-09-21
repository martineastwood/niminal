#pragma once

#include <map>
#include <span>
#include <string>
#include <string_view>

namespace niminal {

struct ProviderSpec {
  std::string_view name;
  std::string_view endpoint;
  std::string_view default_model;
  std::string_view key_hint;
  bool session_routing = false;
  bool stream_usage = false;
  bool apply_cache = false;
  bool prompt_cache_key = false;
};

std::span<const ProviderSpec> all_providers();
const ProviderSpec* find_provider(std::string_view name);
std::string provider_names();
std::string infer_provider(std::string_view api_url);
std::string read_api_key(const ProviderSpec& provider);
std::map<std::string, std::string> provider_headers(const ProviderSpec& provider);

} // namespace niminal
