#include <niminal/agent.hpp>
#include <niminal/openai.hpp>
#include <niminal/text.hpp>

#include <chrono>
#include <future>
#include <thread>
#include <utility>

namespace niminal {

namespace {

constexpr int kMaxEmptyResponses = 1;
constexpr std::string_view kEmptyResponseFollowup =
    "Your previous response ended after internal reasoning\n"
    "without a user-facing answer. Continue now with the answer the user requested.\n"
    "Do not stop after thinking; provide the plan or explanation in your final response.";

bool blank(std::string_view text) {
  return text.find_first_not_of(" \t\r\n") == std::string_view::npos;
}

json tools_payload(const std::vector<Tool>& tools) {
  json out = json::array();
  for (const auto& tool : tools) {
    out.push_back({
        {"type", "function"},
        {"function",
         {{"name", tool.name}, {"description", tool.description}, {"parameters", tool.parameters}}},
    });
  }
  return out;
}

const Tool* find_tool(const std::vector<Tool>& tools, std::string_view name) {
  for (const auto& tool : tools) {
    if (tool.name == name) {
      return &tool;
    }
  }
  return nullptr;
}

bool looks_overflow(std::string_view msg) {
  const std::string s = lower_copy(std::string(msg));
  return s.find("context length") != std::string::npos ||
         s.find("context window") != std::string::npos ||
         s.find("maximum context") != std::string::npos ||
         s.find("too many tokens") != std::string::npos ||
         s.find("prompt is too long") != std::string::npos ||
         (s.find("context") != std::string::npos && s.find("overflow") != std::string::npos);
}

bool looks_transient(std::string_view msg) {
  const auto text = lower_copy(std::string(msg));
  return text.rfind("http 5", 0) == 0 || text.find("http 429:") != std::string::npos ||
         text.find("failure when receiving") != std::string::npos ||
         text.find("failed sending") != std::string::npos ||
         text.find("couldn't connect") != std::string::npos ||
         text.find("could not connect") != std::string::npos ||
         text.find("connection refused") != std::string::npos ||
         text.find("connection reset") != std::string::npos ||
         text.find("timed out") != std::string::npos ||
         text.find("timeout was reached") != std::string::npos ||
         text.find("server returned nothing") != std::string::npos ||
         text.find("transferred a partial file") != std::string::npos;
}

bool wait_for_retry(std::atomic<bool>* cancel, std::chrono::milliseconds delay) {
  const auto deadline = std::chrono::steady_clock::now() + delay;
  while (std::chrono::steady_clock::now() < deadline) {
    if (cancel != nullptr && cancel->load()) {
      return false;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
  }
  return cancel == nullptr || !cancel->load();
}

json parse_tool_input(const std::string& arguments) {
  if (arguments.empty()) {
    return json::object();
  }
  try {
    auto input = json::parse(arguments);
    return input.is_object() ? input : json::object();
  } catch (...) {
    return json::object();
  }
}

} // namespace

json Agent::request_messages() const {
  json out = json::array();
  json parts = json::array();
  auto add_part = [&](const std::string& text) {
    if (text.empty()) {
      return;
    }
    json part = json::object();
    part["type"] = "text";
    part["text"] = text;
    parts.push_back(std::move(part));
  };
  add_part(system);
  for (const auto& block : system_extra) {
    add_part(block);
  }
  if (!parts.empty()) {
    json sys = json::object();
    sys["role"] = "system";
    sys["content"] = std::move(parts);
    out.push_back(std::move(sys));
  }
  if (messages.is_array()) {
    for (const auto& msg : messages) {
      if (msg.is_object() && msg.value("role", "") == "system") {
        continue;
      }
      out.push_back(msg);
    }
  }
  return out;
}

void Agent::fill_chat(ChatRequest& req) const {
  req.api_url = api_url;
  req.api_key = api_key;
  req.model = model;
  req.provider = provider;
  req.key_hint = key_hint;
  req.conversation_id = conversation_id;
  req.extra_headers = extra_headers;
  req.session_routing = session_routing;
  req.stream_usage = stream_usage;
  req.apply_cache = apply_cache;
  req.prompt_cache_key = prompt_cache_key;
  req.extra = extra;
  req.on_event = on_event;
  req.cancel = cancel;
}

std::string Agent::run(UserInput prompt, bool append_user) {
  if (append_user && prepare_user) {
    prompt = prepare_user(std::move(prompt));
  }
  if (system_extra_loader) {
    system_extra = system_extra_loader();
  }
  if (!messages.is_array()) {
    messages = json::array();
  }
  const std::string active_run_id = run_id.empty()
                                        ? (conversation_id.empty() ? "session" : conversation_id) +
                                              ":turn:" + std::to_string(messages.size())
                                        : run_id;
  int step = -1;
  auto emit = [&](StreamEvent event) {
    if (event.run_id.empty()) {
      event.run_id = active_run_id;
    }
    if (event.session_id.empty()) {
      event.session_id = conversation_id;
    }
    if (event.turn_id.empty()) {
      event.turn_id = active_run_id;
    }
    if (event.step < 0 && step >= 0) {
      event.step = step;
    }
    if (event.model.empty()) {
      event.model = model;
    }
    if (on_event) {
      on_event(std::move(event));
    }
  };
  auto user_content = [](const UserInput& input) {
    if (input.images.empty()) {
      return json(input.text);
    }
    json parts = json::array();
    if (!input.text.empty()) {
      parts.push_back(json{{"type", "text"}, {"text", input.text}});
    }
    for (const auto& image : input.images) {
      parts.push_back(image);
    }
    return parts;
  };
  if (append_user) {
    messages.push_back(json{{"role", "user"}, {"content", user_content(prompt)}});
    if (persist_user) {
      persist_user(prompt);
    }
  } else if (messages.empty() || (messages.back().value("role", "") != "user" &&
                                  messages.back().value("role", "") != "tool")) {
    throw Error("nothing to retry");
  }
  emit(StreamEvent{EventKind::run_start, prompt.text, {}, {}});
  if (turn_start) {
    turn_start();
  }
  bool turn_finished = false;
  auto finish_turn = [&](bool interrupted) {
    if (turn_finished) {
      return;
    }
    turn_finished = true;
    if (turn_end) {
      turn_end(interrupted);
    }
  };

  const json tools_json = tools_payload(tools);
  auto inject_steering = [&]() -> int {
    if (!take_steering) {
      return 0;
    }
    int n = 0;
    for (auto input : take_steering()) {
      if (prepare_user) {
        input = prepare_user(std::move(input));
      }
      if (input.text.empty() && input.images.empty()) {
        continue;
      }
      messages.push_back(json{{"role", "user"}, {"content", user_content(input)}});
      if (persist_user) {
        persist_user(input);
      }
      emit(StreamEvent{EventKind::user, input.text, {}, {}});
      ++n;
    }
    return n;
  };
  auto inject_follow_up = [&]() -> int {
    if (!take_follow_up) {
      return 0;
    }
    int n = 0;
    for (auto input : take_follow_up()) {
      if (prepare_user) {
        input = prepare_user(std::move(input));
      }
      if (input.text.empty() && input.images.empty()) {
        continue;
      }
      messages.push_back(json{{"role", "user"}, {"content", user_content(input)}});
      if (persist_user) {
        persist_user(input);
      }
      emit(StreamEvent{EventKind::user, input.text, {}, {}});
      ++n;
    }
    return n;
  };

  try {
    bool overflow_retried = false;
    int empty_responses = 0;
    bool empty_response_followup_pending = false;
    for (step = 0; max_steps <= 0 || step < max_steps; ++step) {
      if (cancelled()) {
        throw Cancelled();
      }
      inject_steering();
      if (before_request) {
        before_request();
      }

      ChatRequest req;
      fill_chat(req);
      req.messages = request_messages();
      if (empty_response_followup_pending) {
        req.messages.push_back(json{{"role", "user"}, {"content", kEmptyResponseFollowup}});
        empty_response_followup_pending = false;
      }
      if (augment_context) {
        augment_context(req.messages);
      }
      req.tools = tools_json;
      emit(StreamEvent{EventKind::step_start, {}, {}, {}});
      req.on_event = [&, step](const StreamEvent& event) {
        auto tagged = event;
        tagged.step = step;
        tagged.model = model;
        emit(std::move(tagged));
      };
      ChatResult result;
      constexpr int kMaxRetries = 3;
      int retries = 0;
      bool restart_step = false;
      while (true) {
        try {
          result = stream_chat_fn ? stream_chat_fn(req) : niminal::stream_chat(req);
          break;
        } catch (const Error& e) {
          if (!overflow_retried && looks_overflow(e.what()) && recover_overflow &&
              recover_overflow()) {
            overflow_retried = true;
            --step;
            restart_step = true;
            break;
          }
          if (!looks_transient(e.what()) || retries == kMaxRetries) {
            if (retries == kMaxRetries && looks_transient(e.what())) {
              StreamEvent retry{EventKind::status,
                                "Connection failed after " + std::to_string(kMaxRetries) +
                                    " retries.",
                                {},
                                {}};
              retry.retry = true;
              emit(std::move(retry));
            }
            throw;
          }
          ++retries;
          StreamEvent retry{EventKind::status,
                            "Connection lost; retrying " + std::to_string(retries) + "/" +
                                std::to_string(kMaxRetries) + "…",
                            {},
                            {}};
          retry.retry = true;
          emit(std::move(retry));
          if (!wait_for_retry(cancel, std::chrono::seconds(1 << (retries - 1)))) {
            throw Cancelled();
          }
        }
      }
      if (restart_step) {
        continue;
      }
      overflow_retried = false;

      if (cancelled()) {
        throw Cancelled();
      }

      if (result.tool_calls.empty()) {
        if (persist_assistant) {
          persist_assistant(result.text, result.tool_calls, model, result.usage,
                            result.reasoning_content, result.reasoning_details);
        }
        StreamEvent assistant{EventKind::assistant_message, result.text, {}, {}};
        assistant.final = true;
        emit(std::move(assistant));
        StreamEvent step_end{EventKind::step_end, {}, {}, {}};
        step_end.usage = result.usage;
        emit(std::move(step_end));
        if (blank(result.text)) {
          if (empty_responses < kMaxEmptyResponses) {
            ++empty_responses;
            emit(StreamEvent{EventKind::status,
                             "The model returned no user-facing answer; asking it "
                             "to finish…",
                             {},
                             {}});
            empty_response_followup_pending = true;
            continue;
          }
          emit(StreamEvent{
              EventKind::error, "The model stopped without a user-facing answer.", {}, {}});
        }
        if (inject_steering() > 0) {
          continue;
        }
        if (inject_follow_up() > 0) {
          continue;
        }
        finish_turn(false);
        emit(StreamEvent{EventKind::run_end, {}, {}, {}});
        emit(StreamEvent{EventKind::done, {}, {}, {}});
        return result.text;
      }

      empty_responses = 0;
      json assistant = {{"role", "assistant"}, {"content", result.text}};
      if (!result.reasoning_content.empty()) {
        assistant["reasoning_content"] = result.reasoning_content;
      }
      if (!result.reasoning_details.empty()) {
        assistant["reasoning_details"] = result.reasoning_details;
      }
      json calls = json::array();
      if (!result.text.empty()) {
        StreamEvent message{EventKind::assistant_message, result.text, {}, {}};
        message.final = false;
        emit(std::move(message));
      }
      for (const auto& call : result.tool_calls) {
        calls.push_back({
            {"id", call.id},
            {"type", "function"},
            {"function", {{"name", call.name}, {"arguments", call.arguments}}},
        });
        StreamEvent tool_call{EventKind::tool_call, call.arguments, call.name, call.id};
        tool_call.input = parse_tool_input(call.arguments);
        emit(std::move(tool_call));
      }
      assistant["tool_calls"] = std::move(calls);
      messages.push_back(std::move(assistant));
      if (persist_assistant) {
        persist_assistant(result.text, result.tool_calls, model, result.usage,
                          result.reasoning_content, result.reasoning_details);
      }

      struct ToolExecution {
        ToolResult output;
        bool is_error = false;
      };
      auto run_tool = [&](const ToolCall& call) -> ToolExecution {
        if (cancelled()) {
          return {"interrupted", true};
        }
        try {
          json args = json::object();
          if (!call.arguments.empty()) {
            args = json::parse(call.arguments);
          }
          const Tool* tool = find_tool(tools, call.name);
          if (!tool) {
            return {"unknown tool: " + call.name, true};
          }
          if (approve_tool && !approve_tool(call, *tool)) {
            return {"approval_denied: Tool execution was denied.", true};
          }
          std::string reason;
          if (before_tool && !before_tool(call, args, reason)) {
            return {"approval_denied: " +
                        (reason.empty() ? std::string("blocked by extension") : reason),
                    true};
          }
          ToolResult output;
          bool is_error = false;
          try {
            if (!tool_output) {
              output = tool->run(args);
            } else {
              struct ToolOutputScope {
                std::function<void(std::string)>& slot;
                std::function<void(std::string)> previous;
                ~ToolOutputScope() { slot = std::move(previous); }
              } output_scope{*tool_output, *tool_output};
              *tool_output = [&](std::string snapshot) {
                emit(StreamEvent{EventKind::tool_output_delta, std::move(snapshot), call.name,
                                 call.id});
              };
              output = tool->run(args);
            }
            is_error = output.text == "interrupted" || output.text.rfind("tool error:", 0) == 0 ||
                       output.text.rfind("unknown tool:", 0) == 0 ||
                       output.text.rfind("approval_denied:", 0) == 0;
          } catch (const std::exception& e) {
            output = std::string("tool error: ") + e.what();
            is_error = true;
          }
          if (after_tool) {
            after_tool(call, args, output.text, is_error);
          }
          return {std::move(output), is_error};
        } catch (const std::exception& e) {
          return {std::string("tool error: ") + e.what(), true};
        }
      };

      auto apply_tool_result = [&](const ToolCall& call, const ToolExecution& execution) {
        StreamEvent tool_result{EventKind::tool_result, execution.output.text, call.name, call.id};
        tool_result.is_error = cancelled() || execution.is_error;
        const bool is_error = tool_result.is_error;
        emit(std::move(tool_result));
        if (persist_tool) {
          persist_tool(call.id, execution.output, is_error);
        }
        messages.push_back(json{
            {"role", "tool"},
            {"tool_call_id", call.id},
            {"content", execution.output.text},
            {"images", execution.output.images},
        });
      };

      const auto& pending = result.tool_calls;
      for (size_t i = 0; i < pending.size();) {
        size_t j = i;
        while (j < pending.size()) {
          const Tool* tool = find_tool(tools, pending[j].name);
          if ((tool == nullptr) || !tool->read_only || (approve_tool && tool->extension)) {
            break;
          }
          ++j;
        }
        const size_t run_len = j - i;
        if (run_len >= 2) {
          std::vector<std::future<ToolExecution>> futures;
          futures.reserve(run_len);
          for (size_t k = i; k < j; ++k) {
            futures.push_back(std::async(std::launch::async, run_tool, pending[k]));
          }
          for (size_t k = 0; k < run_len; ++k) {
            apply_tool_result(pending[i + k], futures[k].get());
          }
          i = j;
        } else {
          apply_tool_result(pending[i], run_tool(pending[i]));
          ++i;
        }
      }
      StreamEvent step_end{EventKind::step_end, {}, {}, {}};
      step_end.usage = result.usage;
      emit(std::move(step_end));
    }
    throw Error("Maximum tool-loop steps reached (" + std::to_string(max_steps) +
                "). The session is saved; continue "
                "the task or rerun with --max-steps N.");
  } catch (const Cancelled&) {
    finish_turn(true);
    emit(StreamEvent{EventKind::error, "interrupted", {}, {}});
    emit(StreamEvent{EventKind::done, {}, {}, {}});
    return {};
  } catch (...) {
    finish_turn(false);
    throw;
  }
}

} // namespace niminal
