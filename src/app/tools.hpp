#pragma once

#include "workspace.hpp"

#include <niminal/types.hpp>

#include <atomic>
#include <functional>
#include <map>
#include <string>
#include <vector>

namespace niminal::app {

using ShellEnv = std::map<std::string, std::string>;
using ShellEnvFn = std::function<ShellEnv()>;

std::vector<niminal::Tool>
workspace_tools(Workspace& ws, std::atomic<bool>* cancel = nullptr,
                const std::function<void(const std::string&)>& on_bash_output = {},
                const ShellEnvFn* shell_env = nullptr);

} // namespace niminal::app
