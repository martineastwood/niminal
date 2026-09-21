#pragma once

#include "workspace.hpp"

#include <niminal/types.hpp>

#include <atomic>
#include <functional>
#include <string>
#include <vector>

namespace niminal::app {

std::vector<niminal::Tool>
workspace_tools(Workspace& ws, std::atomic<bool>* cancel = nullptr,
                const std::function<void(const std::string&)>& on_bash_output = {});

} // namespace niminal::app
