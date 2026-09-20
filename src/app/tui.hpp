#pragma once

#include <niminal/agent.hpp>
#include "config.hpp"
#include "extensions.hpp"
#include "session.hpp"
#include "workspace.hpp"

#include <filesystem>
#include <string>
#include <vector>

namespace niminal::app {

int run_tui(niminal::Agent& agent, Workspace& workspace,
            Config& cfg, Session& session,
            std::shared_ptr<ExtensionRuntime>& extensions,
            bool yolo = false,
            const std::vector<std::string>* allowed_tools = nullptr);

}  // namespace niminal::app
