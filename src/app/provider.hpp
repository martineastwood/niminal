#pragma once

#include "config.hpp"

#include <niminal/agent.hpp>

#include <string>
#include <string_view>
#include <vector>

namespace niminal::app {

struct ProviderSpec {
  const char* name;
  const char* endpoint;
  const char* default_model;
  bool session_routing = false;
  bool stream_usage = false;
  bool apply_cache = false;
  bool prompt_cache_key = false;
};

const ProviderSpec* find_provider(std::string_view name);
std::vector<const ProviderSpec*> all_providers();
std::string provider_names();
std::string infer_provider(std::string_view api_url);
std::string read_api_key(const ProviderSpec& spec);
void apply_provider(niminal::Agent& agent, const Config& cfg);
void normalize_config(Config& cfg);
niminal::Result<void> select_provider(Config& cfg, std::string_view name);

}  // namespace niminal::app
