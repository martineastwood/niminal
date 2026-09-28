#include "json_mode.hpp"

#include <iostream>

using niminal::EventKind;
using niminal::json;
using niminal::StreamEvent;

namespace {

int fail(const char* message) {
  std::cerr << message << '\n';
  return 1;
}

} // namespace

int main() {
  StreamEvent start{EventKind::run_start, "inspect the repo", {}, {}};
  start.session_id = "session";
  start.turn_id = "session:turn:0";
  start.run_id = start.turn_id;
  auto start_json = niminal::app::json_event(start);
  if (niminal::json_value(start_json, "version", 0) != 1 ||
      niminal::json_value(start_json, "type", "") != "run_start" ||
      niminal::json_value(start_json, "prompt", "") != "inspect the repo") {
    return fail("run_start JSON mismatch");
  }

  StreamEvent call{EventKind::tool_call, "{\"path\":\"README.md\"}", "read", "t1"};
  call.step = 0;
  call.input = json{{"path", "README.md"}};
  auto call_json = niminal::app::json_event(call);
  if (niminal::json_value(call_json, "type", "") != "tool_call" ||
      niminal::json_value(call_json, "tool_id", "") != "t1" ||
      !niminal::json_equal(call_json["input"], json{{"path", "README.md"}})) {
    return fail("tool_call JSON mismatch");
  }

  StreamEvent output{EventKind::tool_output_delta, "compiling src/app/tui.cpp", "bash", "t2"};
  output.step = 0;
  auto output_json = niminal::app::json_event(output);
  if (niminal::json_value(output_json, "type", "") != "tool_output_delta" ||
      niminal::json_value(output_json, "tool_id", "") != "t2" ||
      niminal::json_value(output_json, "tool_name", "") != "bash" ||
      niminal::json_value(output_json, "delta", "") != "compiling src/app/tui.cpp") {
    return fail("tool_output_delta JSON mismatch");
  }

  StreamEvent end{EventKind::step_end, {}, {}, {}};
  end.step = 0;
  end.model = "test-model";
  end.usage.input_tokens = 12;
  end.usage.output_tokens = 3;
  auto end_json = niminal::app::json_event(end);
  if (niminal::json_value(end_json, "type", "") != "step_end" ||
      niminal::json_value(end_json["usage"], "input_tokens", 0) != 12 ||
      niminal::json_value(end_json["usage"], "output_tokens", 0) != 3) {
    return fail("step_end JSON mismatch");
  }

  auto message =
      niminal::app::message_event("session", "turn", "assistant", "done", "test-model", true);
  if (niminal::json_value(message, "type", "") != "message" ||
      !niminal::json_value(message, "final", false)) {
    return fail("message JSON mismatch");
  }

  auto queue = niminal::app::queue_event("session", "enqueue", 2, "do this", "request", "steer");
  if (niminal::json_value(queue, "type", "") != "queue" ||
      niminal::json_value(queue, "depth", 0) != 2 ||
      niminal::json_value(queue, "request_id", "") != "request" ||
      niminal::json_value(queue, "mode", "") != "steer") {
    return fail("queue JSON mismatch");
  }

  StreamEvent interrupted{EventKind::interrupted, "Interrupted.", {}, {}};
  auto interrupted_json = niminal::app::json_event(interrupted);
  if (niminal::json_value(interrupted_json, "type", "") != "interrupted" ||
      niminal::json_value(interrupted_json, "message", "") != "Interrupted.") {
    return fail("interrupted JSON mismatch");
  }
  return 0;
}
