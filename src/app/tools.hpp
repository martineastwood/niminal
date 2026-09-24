#pragma once

#include "config.hpp"
#include "session.hpp"
#include "workspace.hpp"

#include <niminal/agent.hpp>
#include <niminal/types.hpp>

#include <atomic>
#include <fcntl.h>
#include <functional>
#include <map>
#include <string>
#include <string_view>
#include <vector>

namespace niminal::app {

using ShellEnv = std::map<std::string, std::string>;
using ShellEnvFn = std::function<ShellEnv()>;

ShellEnv make_shell_env(const Session& session, const niminal::Agent& agent, const Config& cfg);

// Keeps a pipe end out of processes that only exist to run a command: without
// this, every extension inherits its siblings' pipes and never sees stdin close
// when niminal exits, so it cannot clean up the children it started.
inline void close_on_exec(int fd) {
  fcntl(fd, F_SETFD, FD_CLOEXEC);
}

// The environment for a child process: the session block plus the inherited
// environment, with any inherited entry whose name is in the block dropped so
// the session values always win.
inline std::vector<std::string> child_environment(const ShellEnv& env, char** inherited) {
  std::vector<std::string> entries;
  entries.reserve(env.size());
  for (const auto& [key, value] : env) {
    entries.push_back(key + "=" + value);
  }
  for (char** e = inherited; e != nullptr && *e != nullptr; ++e) {
    const std::string_view entry(*e);
    const auto eq = entry.find('=');
    if (eq != std::string_view::npos && env.contains(std::string(entry.substr(0, eq)))) {
      continue;
    }
    entries.emplace_back(entry);
  }
  return entries;
}

std::string run_bash(const std::string& command, const std::filesystem::path& cwd, int timeout_s,
                     std::atomic<bool>* cancel,
                     const std::function<void(const std::string&)>& on_output, const ShellEnv& env);

std::vector<niminal::Tool>
workspace_tools(Workspace& ws, std::atomic<bool>* cancel = nullptr,
                const std::function<void(const std::string&)>& on_bash_output = {},
                const ShellEnvFn* shell_env = nullptr);

} // namespace niminal::app
