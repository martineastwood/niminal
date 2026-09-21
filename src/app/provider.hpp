#pragma once

#include "config.hpp"
#include "session.hpp"

#include <niminal/agent.hpp>
#include <niminal/providers.hpp>

#include <string_view>

namespace niminal::app {

void apply_provider(niminal::Agent& agent, const Config& cfg);
void normalize_config(Config& cfg);
niminal::Result<void> select_provider(Config& cfg, std::string_view name);
void restore_config_from_session(Config& cfg, const Session& session, bool restore_provider = true,
                                 bool restore_model = true);

} // namespace niminal::app
