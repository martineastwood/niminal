#include "json_mode.hpp"

#include "json_events.hpp"

namespace niminal::app {
namespace {

void add_identity(nlohmann::json& out, const niminal::StreamEvent& event) {
  if (!event.session_id.empty()) {
    out["session_id"] = event.session_id;
  }
  if (!event.turn_id.empty()) {
    out["turn_id"] = event.turn_id;
  }
  if (!event.run_id.empty()) {
    out["run_id"] = event.run_id;
  }
}

void add_usage(nlohmann::json& out, const niminal::Usage& usage) {
  if (usage.input_tokens == 0 && usage.output_tokens == 0 && !usage.cache_reported) {
    return;
  }
  out["usage"] = {
      {"input_tokens", usage.input_tokens},
      {"output_tokens", usage.output_tokens},
      {"cache_read_tokens", usage.cache_read_tokens},
      {"cache_write_tokens", usage.cache_write_tokens},
      {"cache_reported", usage.cache_reported},
  };
}

} // namespace

nlohmann::json json_event(const niminal::StreamEvent& event) {
  nlohmann::json out = {{"version", kJsonEventVersion}};
  add_identity(out, event);
  switch (event.kind) {
  case niminal::EventKind::run_start:
    out["type"] = "run_start";
    if (!event.text.empty()) {
      out["prompt"] = event.text;
    }
    break;
  case niminal::EventKind::step_start:
    out["type"] = "step_start";
    out["step"] = event.step;
    if (!event.model.empty()) {
      out["model"] = event.model;
    }
    break;
  case niminal::EventKind::text_delta:
    out["type"] = "message_delta";
    out["step"] = event.step;
    out["delta"] = event.text;
    if (!event.model.empty()) {
      out["model"] = event.model;
    }
    break;
  case niminal::EventKind::thinking_delta:
    out["type"] = "thinking_delta";
    out["step"] = event.step;
    out["delta"] = event.text;
    if (!event.model.empty()) {
      out["model"] = event.model;
    }
    break;
  case niminal::EventKind::tool_call:
    out["type"] = "tool_call";
    out["step"] = event.step;
    out["tool_id"] = event.tool_id;
    out["tool_name"] = event.tool_name;
    if (!event.input.is_null()) {
      out["input"] = event.input;
    }
    break;
  case niminal::EventKind::approval_required:
    out["type"] = "approval_required";
    out["step"] = event.step;
    out["tool_id"] = event.tool_id;
    out["tool_name"] = event.tool_name;
    out["description"] = event.text;
    out["can_remember"] = event.can_remember;
    if (!event.input.is_null()) {
      out["input"] = event.input;
    }
    break;
  case niminal::EventKind::tool_output_delta:
    out["type"] = "tool_output_delta";
    out["step"] = event.step;
    out["tool_id"] = event.tool_id;
    out["tool_name"] = event.tool_name;
    out["delta"] = event.text;
    break;
  case niminal::EventKind::tool_result:
    out["type"] = "tool_result";
    out["step"] = event.step;
    out["tool_id"] = event.tool_id;
    out["tool_name"] = event.tool_name;
    out["output"] = event.text;
    out["is_error"] = event.is_error;
    break;
  case niminal::EventKind::step_end:
    out["type"] = "step_end";
    out["step"] = event.step;
    if (!event.model.empty()) {
      out["model"] = event.model;
    }
    add_usage(out, event.usage);
    break;
  case niminal::EventKind::run_end:
    out["type"] = "run_end";
    if (!event.model.empty()) {
      out["model"] = event.model;
    }
    break;
  case niminal::EventKind::assistant_message:
    return message_event(event.session_id, event.turn_id, "assistant", event.text, event.model,
                         event.final);
  case niminal::EventKind::user:
    return message_event(event.session_id, event.turn_id, "user", event.text);
  case niminal::EventKind::error:
    out["type"] = "error";
    out["step"] = event.step;
    out["message"] = event.text;
    break;
  case niminal::EventKind::status:
  case niminal::EventKind::done:
    return nlohmann::json();
  }
  if (event.duration_ms > 0) {
    out["duration_ms"] = event.duration_ms;
  }
  return out;
}

nlohmann::json session_event(const std::string& type, const std::string& session_id, bool success) {
  nlohmann::json out = {{"version", kJsonEventVersion}, {"type", type}, {"session_id", session_id}};
  if (type == "session_end") {
    out["success"] = success;
  }
  return out;
}

nlohmann::json message_event(const std::string& session_id, const std::string& turn_id,
                             const std::string& role, const std::string& content,
                             const std::string& model, bool final) {
  nlohmann::json out = {
      {"version", kJsonEventVersion},
      {"type", "message"},
      {"role", role},
      {"content", content},
  };
  if (!session_id.empty()) {
    out["session_id"] = session_id;
  }
  if (!turn_id.empty()) {
    out["turn_id"] = turn_id;
  }
  if (!model.empty()) {
    out["model"] = model;
  }
  if (role == "assistant") {
    out["final"] = final;
  }
  return out;
}

nlohmann::json queue_event(const std::string& session_id, const std::string& action, int depth,
                           const std::string& content, const std::string& request_id,
                           const std::string& mode) {
  nlohmann::json out = {{"version", kJsonEventVersion},
                        {"type", "queue"},
                        {"session_id", session_id},
                        {"action", action},
                        {"depth", depth}};
  if (!content.empty()) {
    out["content"] = content;
  }
  if (!request_id.empty()) {
    out["request_id"] = request_id;
  }
  if (!mode.empty()) {
    out["mode"] = mode;
  }
  return out;
}

} // namespace niminal::app
