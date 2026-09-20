#pragma once

#include <niminal/agent.hpp>
#include "config.hpp"
#include "extensions.hpp"
#include "session.hpp"
#include "workspace.hpp"

#include <filesystem>

namespace niminal::app {

int run_tui(niminal::Agent& agent, Workspace& workspace,
            Config& cfg, Session& session,
            std::shared_ptr<ExtensionRuntime>& extensions,
            bool yolo = false);

}  // namespace niminal::app
