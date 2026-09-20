#include <niminal/agent.hpp>
#include <niminal/openai.hpp>

#include <future>
#include <utility>

namespace niminal {

namespace {

json tools_payload(const std::vector<Tool>& tools) {
  json out = json::array();
  for (const auto& tool : tools) {
    out.push_back({
        {"type", "function"},
        {"function",
         {{"name", tool.name},
          {"description", tool.description},
          {"parameters", tool.parameters}}},
    });
  }
  return out;
}

const Tool* find_tool(const std::vector<Tool>& tools, std::string_view name) {
  for (const auto& tool : tools)
    if (tool.name == name) return &tool;
  return nullptr;
}

bool looks_overflow(std::string_view msg) {
  std::string s(msg);
  for (char& c : s)
    if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
  return s.find("context length") != std::string::npos ||
         s.find("context window") != std::string::npos ||
         s.find("maximum context") != std::string::npos ||
         s.find("too many tokens") != std::string::npos ||
         s.find("prompt is too long") != std::string::npos ||
         (s.find("context") != std::string::npos &&
          s.find("overflow") != std::string::npos);
}

}  // namespace

json Agent::request_messages() const {
  json out = json::array();
  json parts = json::array();
  auto add_part = [&](const std::string& text) {
    if (text.empty()) return;
    json part = json::object();
    part["type"] = "text";
    part["text"] = text;
    parts.push_back(std::move(part));
  };
  add_part(system);
  for (const auto& extra : system_extra) add_part(extra);
  if (!parts.empty()) {
    json sys = json::object();
    sys["role"] = "system";
    sys["content"] = std::move(parts);
    out.push_back(std::move(sys));
  }
  if (messages.is_array()) {
    for (const auto& msg : messages) {
      if (msg.is_object() && msg.value("role", "") == "system") continue;
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

std::string Agent::run(const std::string& prompt) {
  if (system_extra_loader) system_extra = system_extra_loader();
  if (!messages.is_array()) messages = json::array();
  messages.push_back(json{{"role", "user"}, {"content", prompt}});
  if (persist_user) persist_user(prompt);

  const json tools_json = tools_payload(tools);
  auto inject_steering = [&]() -> int {
    if (!take_steering) return 0;
    int n = 0;
    for (auto& text : take_steering()) {
      if (text.empty()) continue;
      messages.push_back(json{{"role", "user"}, {"content", text}});
      if (persist_user) persist_user(text);
      if (on_event) on_event(StreamEvent{EventKind::user, text, {}, {}});
      ++n;
    }
    return n;
  };

  try {
    bool overflow_retried = false;
    for (int step = 0; step < max_steps; ++step) {
      if (cancelled()) throw Cancelled();
      inject_steering();
      if (before_request) before_request();

      ChatRequest req;
      fill_chat(req);
      req.messages = request_messages();
      req.tools = tools_json;
      ChatResult result;
      try {
        result = stream_chat(req);
      } catch (const Error& e) {
        if (!overflow_retried && looks_overflow(e.what()) && recover_overflow &&
            recover_overflow()) {
          overflow_retried = true;
          --step;
          continue;
        }
        throw;
      }
      overflow_retried = false;

      if (cancelled()) throw Cancelled();

      if (result.tool_calls.empty()) {
        if (persist_assistant)
          persist_assistant(result.text, result.tool_calls, model, result.usage);
        if (inject_steering() > 0) continue;
        if (on_event)
          on_event(StreamEvent{EventKind::done, {}, {}, {}});
        return result.text;
      }

      json assistant = {{"role", "assistant"}, {"content", result.text}};
      json calls = json::array();
      for (const auto& call : result.tool_calls) {
        calls.push_back({
            {"id", call.id},
            {"type", "function"},
            {"function", {{"name", call.name}, {"arguments", call.arguments}}},
        });
        if (on_event) {
          on_event(StreamEvent{EventKind::tool_call, call.arguments, call.name,
                               call.id});
        }
      }
      assistant["tool_calls"] = std::move(calls);
      messages.push_back(std::move(assistant));
      if (persist_assistant)
        persist_assistant(result.text, result.tool_calls, model, result.usage);

      auto run_tool = [&](const ToolCall& call) -> std::string {
        if (cancelled()) return "interrupted";
        try {
          json args = json::object();
          if (!call.arguments.empty()) args = json::parse(call.arguments);
          const Tool* tool = find_tool(tools, call.name);
          if (!tool) return "unknown tool: " + call.name;
          return tool->run(args);
        } catch (const std::exception& e) {
          return std::string("tool error: ") + e.what();
        }
      };

      auto apply_tool_result = [&](const ToolCall& call,
                                   const std::string& output) {
        if (on_event) {
          on_event(
              StreamEvent{EventKind::tool_result, output, call.name, call.id});
        }
        if (persist_tool) {
          bool err = cancelled() || output == "interrupted" ||
                     output.rfind("tool error:", 0) == 0 ||
                     output.rfind("unknown tool:", 0) == 0;
          persist_tool(call.id, output, err);
        }
        messages.push_back(json{
            {"role", "tool"},
            {"tool_call_id", call.id},
            {"content", output},
        });
      };

      const auto& pending = result.tool_calls;
      for (size_t i = 0; i < pending.size();) {
        size_t j = i;
        while (j < pending.size()) {
          const Tool* tool = find_tool(tools, pending[j].name);
          if (!tool || !tool->read_only) break;
          ++j;
        }
        const size_t run_len = j - i;
        if (run_len >= 2) {
          std::vector<std::future<std::string>> futures;
          futures.reserve(run_len);
          for (size_t k = i; k < j; ++k)
            futures.push_back(
                std::async(std::launch::async, run_tool, pending[k]));
          for (size_t k = 0; k < run_len; ++k)
            apply_tool_result(pending[i + k], futures[k].get());
          i = j;
        } else {
          apply_tool_result(pending[i], run_tool(pending[i]));
          ++i;
        }
      }
    }
    throw Error("max_steps reached (" + std::to_string(max_steps) + ")");
  } catch (const Cancelled&) {
    if (on_event) {
      on_event(StreamEvent{EventKind::error, "interrupted", {}, {}});
      on_event(StreamEvent{EventKind::done, {}, {}, {}});
    }
    return {};
  }
}

}  // namespace niminal
