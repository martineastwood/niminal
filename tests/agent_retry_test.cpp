#include <niminal/agent.hpp>
#include <niminal/chat.hpp>

#include <cail/opencode.hpp>

#include <atomic>
#include <barrier>
#include <iostream>

namespace {

class StreamTransport final : public cail::HttpTransport {
public:
  std::function<std::string(const cail::HttpRequest&)> response;

  cail::Result<cail::HttpResponse> send(const cail::HttpRequest&) override {
    throw niminal::Error("expected a streaming request");
  }

  cail::Result<cail::HttpResponse> stream(const cail::HttpRequest& request,
                                          const cail::HttpDataHandler& on_data,
                                          std::stop_token) override {
    on_data(response(request));
    return cail::HttpResponse{.status_code = 200, .headers = {}, .body = {}};
  }
};

} // namespace

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

  niminal::Agent rejected;
  rejected.model = "test-model";
  int rejected_requests = 0;
  rejected.stream_chat_fn = [&](const niminal::ChatRequest&) -> niminal::ChatResult {
    ++rejected_requests;
    throw niminal::Error("http 400: ", 400);
  };
  try {
    rejected.run("hello");
    std::cerr << "an unexplained HTTP 400 should fail\n";
    return 1;
  } catch (const niminal::Error& error) {
    if (rejected_requests != 1 || error.http_status != 400 ||
        std::string_view(error.what()).find("/compact") != std::string_view::npos) {
      std::cerr << "an unexplained HTTP 400 should not retry or suggest compaction\n";
      return 1;
    }
  }

  niminal::Agent permanent;
  permanent.model = "test-model";
  permanent.stream_chat_fn = [](const niminal::ChatRequest&) -> niminal::ChatResult {
    throw niminal::Error("http 400: invalid model", 400);
  };
  try {
    permanent.run("hello");
    std::cerr << "a detailed HTTP 400 should fail\n";
    return 1;
  } catch (const niminal::Error& error) {
    if (std::string_view(error.what()) != "http 400: invalid model") {
      std::cerr << "a detailed HTTP 400 should preserve the provider message\n";
      return 1;
    }
  }

  for (const auto& error : {
           niminal::json{
               {"message", "Streaming response failed: [server_error] upstream service timeout"}},
           niminal::json{{"message", "upstream service timeout"}, {"code", "server_error"}},
           niminal::json{{"message", "upstream service timeout"}, {"type", "server_error"}},
       }) {
    niminal::Agent opencode;
    opencode.model = "glm-5.3-flash";
    opencode.conversation_id = "test-session";
    int requests = 0;
    int tool_runs = 0;
    int saved_users = 0;
    int retry_events = 0;
    std::string failed_request;
    opencode.persist_user = [&](const niminal::UserInput&) { ++saved_users; };
    opencode.on_event = [&](const niminal::StreamEvent& event) {
      if (event.retry) {
        ++retry_events;
      }
    };
    opencode.tools.push_back(
        niminal::Tool{"lookup",
                      "lookup",
                      {{"type", "object"}, {"properties", niminal::json_object()}},
                      [&](const niminal::json&) {
                        ++tool_runs;
                        return "found";
                      }});
    auto transport = std::make_unique<StreamTransport>();
    transport->response = [&](const cail::HttpRequest& request) -> std::string {
      if (++requests == 1) {
        return "data: "
               R"({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"lookup","arguments":"{}"}}]},"finish_reason":"tool_calls"}]})"
               "\n\ndata: [DONE]\n\n";
      }
      if (requests == 2) {
        failed_request = request.body;
        return "data: "
               R"({"choices":[{"index":0,"delta":{"content":"partial"}}]})"
               "\n\ndata: " +
               niminal::json_dump(niminal::json{{"error", error}}) + "\n\n";
      }
      if (request.body != failed_request) {
        throw niminal::Error("retry changed the provider request");
      }
      return "data: "
             R"({"choices":[{"index":0,"delta":{"content":"recovered"},"finish_reason":"stop"}]})"
             "\n\ndata: [DONE]\n\n";
    };
    opencode.language_model = cail::create_opencode(
        {.api_key = "test", .service = cail::OpenCodeService::go, .base_url = "http://unused"})(
        opencode.model, cail::OpenCodeApiFamily::chat_completions, std::move(transport));
    if (opencode.run("look it up") != "recovered" || requests != 3 || tool_runs != 1 ||
        saved_users != 1 || retry_events != 1) {
      std::cerr << "OpenCode stream failures should retry without repeating completed tools\n";
      return 1;
    }
  }

  for (const auto* code : {"invalid_request_error", "authentication_error"}) {
    auto transport = std::make_unique<StreamTransport>();
    transport->response = [code](const cail::HttpRequest&) {
      return "data: " +
             niminal::json_dump(
                 niminal::json{{"error", {{"message", "request rejected"}, {"type", code}}}}) +
             "\n\n";
    };
    niminal::ChatRequest request;
    request.conversation_id = "test-session";
    request.messages = niminal::json_array({{{"role", "user"}, {"content", "hello"}}});
    request.model = cail::create_opencode(
        {.api_key = "test", .service = cail::OpenCodeService::go, .base_url = "http://unused"})(
        "glm-5.3-flash", cail::OpenCodeApiFamily::chat_completions, std::move(transport));
    try {
      niminal::stream_chat(request);
      std::cerr << "permanent stream errors should fail\n";
      return 1;
    } catch (const niminal::Error& error) {
      if (error.retryable || error.http_status != 200) {
        std::cerr << "permanent stream errors should retain status without becoming retryable: "
                  << error.what() << '\n';
        return 1;
      }
    }
  }

  niminal::Agent reasoning_agent;
  reasoning_agent.model = "thinking-model";
  reasoning_agent.tools.push_back(niminal::Tool{"lookup", "lookup", niminal::json_object(),
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
      const auto options =
          niminal::json_value(assistant, "provider_options", niminal::json_object());
      if (niminal::json_value(options, "reasoning_content", "") != "need lookup" ||
          !niminal::json_equal(
              niminal::json_value(options, "reasoning_details", niminal::json_array()),
              niminal::json_array({{{"type", "reasoning.text"}, {"text", "need lookup"}}}))) {
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
    parallel_agent.tools.push_back(niminal::Tool{name, name, niminal::json_object(),
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
