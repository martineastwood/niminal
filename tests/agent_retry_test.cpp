#include <niminal/agent.hpp>
#include <niminal/openai.hpp>

#include <iostream>

int main() {
  niminal::Agent agent;
  agent.api_key = "test";
  agent.model = "test-model";

  int calls = 0;
  int persisted_users = 0;
  int retries = 0;
  agent.persist_user = [&](const std::string&) { ++persisted_users; };
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
      throw niminal::Error("http: Failure when receiving data from the peer");
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
  return 0;
}
