#include "rpc.hpp"

namespace niminal::app {

namespace {
constexpr int kJsonEventVersion = 1;
}  // namespace

bool valid_rpc_queue_mode(std::string_view mode) {
  return mode == "all" || mode == "one-at-a-time";
}

nlohmann::json rpc_response_event(const std::string& id, bool ok,
                                  const std::string& state,
                                  const std::string& error) {
  nlohmann::json out = {{"version", kJsonEventVersion},
                        {"type", "response"},
                        {"id", id},
                        {"ok", ok}};
  if (!state.empty()) out["state"] = state;
  if (!error.empty()) out["error"] = error;
  return out;
}

}  // namespace niminal::app
