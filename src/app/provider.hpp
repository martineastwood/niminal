#pragma once

#include "config.hpp"

#include <niminal/agent.hpp>
#include <niminal/providers.hpp>

#include <string>
#include <string_view>

namespace niminal::app {

void apply_provider(niminal::Agent& agent, const Config& cfg);
void normalize_config(Config& cfg);
niminal::Result<void> select_provider(Config& cfg, std::string_view name);

} // namespace niminal::app
