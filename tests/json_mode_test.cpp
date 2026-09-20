#include "json_mode.hpp"

#include <iostream>

using nlohmann::json;
using niminal::EventKind;
using niminal::StreamEvent;

namespace {

int fail(const char* message) {
  std::cerr << message << '\n';
  return 1;
}

}  // namespace

int main() {
  StreamEvent start{EventKind::run_start, "inspect the repo", {}, {}};
  start.session_id = "session";
  start.turn_id = "session:turn:0";
  start.run_id = start.turn_id;
  auto start_json = niminal::app::json_event(start);
  if (start_json.value("version", 0) != 1 ||
      start_json.value("type", "") != "run_start" ||
      start_json.value("prompt", "") != "inspect the repo")
    return fail("run_start JSON mismatch");

  StreamEvent call{EventKind::tool_call, "{\"path\":\"README.md\"}", "read", "t1"};
  call.step = 0;
  call.input = json{{"path", "README.md"}};
  auto call_json = niminal::app::json_event(call);
  if (call_json.value("type", "") != "tool_call" ||
      call_json.value("tool_id", "") != "t1" ||
      call_json["input"] != json{{"path", "README.md"}})
    return fail("tool_call JSON mismatch");

  StreamEvent end{EventKind::step_end, {}, {}, {}};
  end.step = 0;
  end.model = "test-model";
  end.usage.input_tokens = 12;
  end.usage.output_tokens = 3;
  auto end_json = niminal::app::json_event(end);
  if (end_json.value("type", "") != "step_end" ||
      end_json["usage"].value("input_tokens", 0) != 12 ||
      end_json["usage"].value("output_tokens", 0) != 3)
    return fail("step_end JSON mismatch");

  auto message = niminal::app::message_event("session", "turn", "assistant",
                                              "done", "test-model", true);
  if (message.value("type", "") != "message" ||
      message.value("final", false) != true)
    return fail("message JSON mismatch");

  auto queue = niminal::app::queue_event("session", "enqueue", 2,
                                         "do this", "request", "steer");
  if (queue.value("type", "") != "queue" || queue.value("depth", 0) != 2 ||
      queue.value("request_id", "") != "request" ||
      queue.value("mode", "") != "steer")
    return fail("queue JSON mismatch");

  auto diagnostic = niminal::app::diagnostic_event("warning", "careful");
  if (diagnostic.value("type", "") != "diagnostic" ||
      diagnostic.value("level", "") != "warning")
    return fail("diagnostic JSON mismatch");
  return 0;
}
