#pragma once

#include "config.hpp"
#include "extensions.hpp"
#include "models_dev.hpp"
#include "session.hpp"
#include "workspace.hpp"
#include <niminal/agent.hpp>

#include <filesystem>
#include <functional>
#include <string>
#include <vector>

namespace niminal::app {

int run_tui(niminal::Agent& agent, Workspace& workspace, Config& cfg, Session& session,
            std::shared_ptr<ExtensionRuntime>& extensions, bool yolo = false,
            const std::vector<std::string>* allowed_tools = nullptr,
            std::function<void()> reload_system_prompt = {},
            CatalogStartup catalog_startup = CatalogStartup::fresh);

} // namespace niminal::app
