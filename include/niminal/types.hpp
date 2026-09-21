#pragma once

#include <expected>
#include <functional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include <nlohmann/json.hpp>

namespace niminal {

using json = nlohmann::json;

struct Error : std::runtime_error {
  using std::runtime_error::runtime_error;
};

struct Cancelled : Error {
  Cancelled() : Error("interrupted") {}
};

template <typename T>
using Result = std::expected<T, Error>;

struct Usage {
  int input_tokens = 0;
  int output_tokens = 0;
  int cache_read_tokens = 0;
  int cache_write_tokens = 0;
  bool cache_reported = false;
};

enum class EventKind {
  text_delta,
  thinking_delta,
  tool_call,
  approval_required,
  tool_output_delta,
  tool_result,
  user,
  status,
  error,
  done,
  run_start,
  step_start,
  step_end,
  run_end,
  assistant_message,
};

struct StreamEvent {
  StreamEvent() = default;
  StreamEvent(EventKind kind, std::string text, std::string tool_name,
              std::string tool_id)
      : kind(kind), text(std::move(text)), tool_name(std::move(tool_name)),
        tool_id(std::move(tool_id)) {}

  EventKind kind = EventKind::text_delta;
  std::string text;
  std::string tool_name;
  std::string tool_id;
  json input;
  bool is_error = false;
  bool final = false;
  bool can_remember = false;
  int step = -1;
  int duration_ms = 0;
  std::string run_id;
  std::string session_id;
  std::string turn_id;
  std::string model;
  Usage usage;
};

struct ToolCall {
  std::string id;
  std::string name;
  std::string arguments;
};

struct Tool {
  std::string name;
  std::string description;
  json parameters;
  std::function<std::string(const json&)> run;
  bool read_only = false;
  bool extension = false;
};

inline void add_usage(Usage& a, const Usage& b) {
  a.input_tokens += b.input_tokens;
  a.output_tokens += b.output_tokens;
  a.cache_read_tokens += b.cache_read_tokens;
  a.cache_write_tokens += b.cache_write_tokens;
  a.cache_reported = a.cache_reported || b.cache_reported;
}

struct ChatResult {
  std::string text;
  std::vector<ToolCall> tool_calls;
  std::string finish_reason;
  Usage usage;
};

}  // namespace niminal
