#include "queue_mode.hpp"
#include "rpc.hpp"

#include <iostream>
#include <string>

namespace {

int fail(const char* message) {
  std::cerr << message << '\n';
  return 1;
}

} // namespace

int main() {
  if (!niminal::app::valid_queue_mode("all")) {
    return fail("all is valid");
  }
  if (!niminal::app::valid_queue_mode("one-at-a-time")) {
    return fail("one-at-a-time is valid");
  }
  if (niminal::app::valid_queue_mode("parallel")) {
    return fail("parallel is invalid");
  }

  auto ok = niminal::app::rpc_response_event("req-1", true, "started");
  if (niminal::json_value(ok, "version", 0) != 1 ||
      niminal::json_value(ok, "type", "") != "response" ||
      niminal::json_value(ok, "id", "") != "req-1" || !niminal::json_value(ok, "ok", false) ||
      niminal::json_value(ok, "state", "") != "started") {
    return fail("started response shape");
  }

  auto err = niminal::app::rpc_response_event("req-2", false, {}, "bad command");
  if (niminal::json_value(err, "ok", true) ||
      niminal::json_value(err, "error", "") != "bad command") {
    return fail("error response shape");
  }
  return 0;
}
