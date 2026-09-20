#pragma once

#include <niminal/agent.hpp>
#include "config.hpp"
#include "session.hpp"
#include "workspace.hpp"

#include <filesystem>

namespace niminal::app {

int run_tui(niminal::Agent& agent, Workspace& workspace,
            Config& cfg, Session& session);

}  // namespace niminal::app
