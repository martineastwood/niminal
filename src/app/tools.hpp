#pragma once

#include "config.hpp"
#include "session.hpp"
#include "workspace.hpp"

#include <niminal/agent.hpp>
#include <niminal/types.hpp>

#include <atomic>
#include <functional>
#include <map>
#include <string>
#include <vector>

namespace niminal::app {

using ShellEnv = std::map<std::string, std::string>;
using ShellEnvFn = std::function<ShellEnv()>;

ShellEnv make_shell_env(const Session& session, const niminal::Agent& agent, const Config& cfg);

std::string run_bash(const std::string& command, const std::filesystem::path& cwd, int timeout_s,
                     std::atomic<bool>* cancel,
                     const std::function<void(const std::string&)>& on_output, const ShellEnv& env);

std::vector<niminal::Tool>
workspace_tools(Workspace& ws, std::atomic<bool>* cancel = nullptr,
                const std::function<void(const std::string&)>& on_bash_output = {},
                const ShellEnvFn* shell_env = nullptr);

} // namespace niminal::app
