#pragma once

#include "config.hpp"
#include "session.hpp"

#include <niminal/agent.hpp>

#include <niminal/json.hpp>

#include <string>

namespace niminal::app {

niminal::json rpc_response_event(const std::string& id, bool ok, const std::string& state = {},
                                 const std::string& error = {});

int run_rpc(niminal::Agent& agent, Session& session, Config& config);

} // namespace niminal::app
