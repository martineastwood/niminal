#pragma once

#include "config.hpp"
#include "session.hpp"

#include <niminal/agent.hpp>

#include <string_view>

namespace niminal::app {

struct CustomProvider {
  std::string name;
  std::string endpoint;
  std::string default_model;
  std::vector<std::string> env_keys;
  bool session_routing = false;
  bool stream_usage = false;
  bool apply_cache = false;
  bool prompt_cache_key = false;
  std::vector<std::string> models;
  bool requires_api_key = true;
  bool operator==(const CustomProvider&) const = default;
};

std::string provider_default_model(std::string_view name);
bool has_provider(std::string_view name);
std::vector<std::string> provider_names();
std::vector<std::string> provider_models(std::string_view name);
bool register_provider(CustomProvider provider);
void unregister_provider(std::string_view name);

void apply_provider(niminal::Agent& agent, Config& cfg, std::string_view api_key = {});
void normalize_config(Config& cfg);
niminal::Result<void> select_provider(Config& cfg, std::string_view name);
void restore_config_from_session(Config& cfg, const Session& session, bool restore_provider = true,
                                 bool restore_model = true);

} // namespace niminal::app
