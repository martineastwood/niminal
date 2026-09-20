#pragma once

#include <functional>
#include <stdexcept>
#include <string>
#include <string_view>
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

enum class EventKind {
  text_delta,
  tool_call,
  tool_result,
  user,
  status,
  error,
  done,
};

struct StreamEvent {
  EventKind kind = EventKind::text_delta;
  std::string text;
  std::string tool_name;
  std::string tool_id;
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
};

struct Usage {
  int input_tokens = 0;
  int output_tokens = 0;
  int cache_read_tokens = 0;
  int cache_write_tokens = 0;
  bool cache_reported = false;
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
