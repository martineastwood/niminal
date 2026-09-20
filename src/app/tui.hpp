#pragma once

#include <niminal/agent.hpp>
#include "config.hpp"
#include "session.hpp"

#include <filesystem>

namespace niminal::app {

int run_tui(niminal::Agent& agent, const std::filesystem::path& cwd,
            Config& cfg, Session& session);

}  // namespace niminal::app
