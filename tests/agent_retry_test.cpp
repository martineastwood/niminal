#include <niminal/agent.hpp>
#include <niminal/chat.hpp>

#include <atomic>
#include <barrier>
#include <iostream>

int main() {
  niminal::Agent agent;
  agent.model = "test-model";

  int calls = 0;
  int persisted_users = 0;
  int retries = 0;
  agent.persist_user = [&](const niminal::UserInput&) { ++persisted_users; };
  agent.on_event = [&](const niminal::StreamEvent& event) {
    if (event.retry) {
      ++retries;
    }
  };
  agent.stream_chat_fn = [&](const niminal::ChatRequest& request) {
    ++calls;
    if (calls < 3) {
      if (request.on_event) {
        request.on_event(niminal::StreamEvent{niminal::EventKind::text_delta, "partial", {}, {}});
      }
      throw niminal::Error("http: Failure when receiving data from the peer", 0, true);
    }
    niminal::ChatResult result;
    result.text = "recovered";
    return result;
  };

  if (agent.run("hello") != "recovered") {
    std::cerr << "retry should return the recovered response\n";
    return 1;
  }
  if (calls != 3 || retries != 2 || persisted_users != 1) {
    std::cerr << "retry should reuse the request without duplicating the user message\n";
    return 1;
  }
  if (agent.run("hello", false) != "recovered" || calls != 4 || persisted_users != 1) {
    std::cerr << "manual retry should reuse the persisted user message\n";
    return 1;
  }

  niminal::Agent reasoning_agent;
  reasoning_agent.model = "thinking-model";
  reasoning_agent.tools.push_back(niminal::Tool{"lookup", "lookup", niminal::json::object(),
                                                [](const niminal::json&) { return "found"; }});
  int steps = 0;
  reasoning_agent.stream_chat_fn = [&](const niminal::ChatRequest& request) {
    niminal::ChatResult result;
    if (steps++ == 0) {
      result.provider_options =
          R"({"reasoning_content":"need lookup","reasoning_details":[{"type":"reasoning.text","text":"need lookup"}]})";
      result.tool_calls.push_back({"call_1", "lookup", "{}"});
    } else {
      const auto& assistant = request.messages[1];
      const auto options = assistant.value("provider_options", niminal::json::object());
      if (options.value("reasoning_content", "") != "need lookup" ||
          options.value("reasoning_details", niminal::json::array()) !=
              niminal::json::array({{{"type", "reasoning.text"}, {"text", "need lookup"}}})) {
        throw niminal::Error("reasoning missing from tool continuation");
      }
      result.text = "done";
    }
    return result;
  };
  if (reasoning_agent.run("find it") != "done" || steps != 2) {
    std::cerr << "tool continuation should preserve assistant reasoning\n";
    return 1;
  }

  niminal::Agent parallel_agent;
  parallel_agent.model = "test-model";
  std::atomic<int> output_calls{0};
  auto output = parallel_agent.tool_output;
  *output = [&](std::string) { ++output_calls; };
  std::barrier ready(2);
  for (const auto* name : {"left", "right"}) {
    parallel_agent.tools.push_back(niminal::Tool{name, name, niminal::json::object(),
                                                 [output, &ready](const niminal::json&) {
                                                   ready.arrive_and_wait();
                                                   (*output)("snapshot");
                                                   return "done";
                                                 },
                                                 true});
  }
  int parallel_steps = 0;
  parallel_agent.stream_chat_fn = [&](const niminal::ChatRequest&) {
    niminal::ChatResult result;
    if (parallel_steps++ == 0) {
      result.tool_calls = {{"left_call", "left", "{}"}, {"right_call", "right", "{}"}};
    } else {
      result.text = "done";
    }
    return result;
  };
  if (parallel_agent.run("parallel") != "done" || output_calls != 2) {
    std::cerr << "parallel read-only tools should not replace the shared output callback\n";
    return 1;
  }
  return 0;
}
