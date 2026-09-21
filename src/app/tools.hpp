#pragma once

#include "workspace.hpp"

#include <niminal/types.hpp>

#include <atomic>
#include <vector>

namespace niminal::app {

std::vector<niminal::Tool> workspace_tools(Workspace& ws, std::atomic<bool>* cancel = nullptr);

} // namespace niminal::app
