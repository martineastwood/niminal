#include "rpc.hpp"

#include "json_events.hpp"

namespace niminal::app {

nlohmann::json rpc_response_event(const std::string& id, bool ok, const std::string& state,
                                  const std::string& error) {
  nlohmann::json out = {
      {"version", kJsonEventVersion}, {"type", "response"}, {"id", id}, {"ok", ok}};
  if (!state.empty()) {
    out["state"] = state;
  }
  if (!error.empty()) {
    out["error"] = error;
  }
  return out;
}

} // namespace niminal::app
