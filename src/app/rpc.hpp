#pragma once

#include "config.hpp"
#include "session.hpp"

#include <niminal/agent.hpp>

#include <nlohmann/json.hpp>

#include <string>
#include <string_view>

namespace niminal::app {

bool valid_rpc_queue_mode(std::string_view mode);
nlohmann::json rpc_response_event(const std::string& id, bool ok,
                                  const std::string& state = {},
                                  const std::string& error = {});

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
