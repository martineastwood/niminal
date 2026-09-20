#pragma once

#include "config.hpp"
#include "session.hpp"

#include <niminal/agent.hpp>

namespace niminal::app {

class RpcRuntime {
 public:
  RpcRuntime(niminal::Agent& agent, Session& session, Config& config);
  int run();

 private:
  niminal::Agent& agent_;
  Session& session_;
  Config& config_;
};

int run_rpc(niminal::Agent& agent, Session& session, Config& config);

}  // namespace niminal::app
