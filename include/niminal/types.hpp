#pragma once

#include <atomic>
#include <chrono>
#include <expected>
#include <functional>
#include <mutex>
#include <optional>
#include <stdexcept>
#include <stop_token>
#include <string>
#include <string_view>
#include <thread>
#include <utility>
#include <vector>

#include <nlohmann/json.hpp>

namespace niminal {

using json = nlohmann::json;

struct Error : std::runtime_error {
  int http_status = 0;
  bool transport = false;

  explicit Error(const std::string& message, int status = 0, bool transport_error = false)
      : std::runtime_error(message), http_status(status), transport(transport_error) {}
};

// Cooperative cancel for a run. request() stops in-flight model calls immediately.
// A signal handler may only call notify_from_signal(); a bridge thread turns that
// into a stop request because request_stop is not safe to call from a signal handler.
struct Cancellation {
  Cancellation()
      : bridge_([this](std::stop_token done) {
          while (!done.stop_requested()) {
            if (flag_.load(std::memory_order_acquire)) {
              std::lock_guard lock(mu_);
              if (flag_.load(std::memory_order_relaxed)) {
                stop_.request_stop();
              }
            }
            std::this_thread::sleep_for(std::chrono::milliseconds(20));
          }
        }) {}

  Cancellation(const Cancellation&) = delete;
  Cancellation& operator=(const Cancellation&) = delete;

  void request() {
    std::lock_guard lock(mu_);
    flag_.store(true, std::memory_order_release);
    stop_.request_stop();
  }

  void notify_from_signal() noexcept { flag_.store(true, std::memory_order_release); }

  void clear() {
    std::lock_guard lock(mu_);
    flag_.store(false, std::memory_order_release);
    stop_ = {};
  }

  bool requested() const noexcept { return flag_.load(std::memory_order_acquire); }

  std::stop_token token() const {
    std::lock_guard lock(mu_);
    return stop_.get_token();
  }

private:
  std::atomic<bool> flag_{false};
  mutable std::mutex mu_;
  std::stop_source stop_;
  std::jthread bridge_;
};

struct Cancelled : Error {
  Cancelled() : Error("interrupted") {}
};

template <typename T> using Result = std::expected<T, Error>;

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
  StreamEvent(EventKind event_kind, std::string event_text, std::string event_tool_name,
              std::string event_tool_id)
      : kind(event_kind), text(std::move(event_text)), tool_name(std::move(event_tool_name)),
        tool_id(std::move(event_tool_id)) {}

  EventKind kind = EventKind::text_delta;
  std::string text;
  std::string tool_name;
  std::string tool_id;
  json input;
  bool is_error = false;
  bool final = false;
  bool can_remember = false;
  bool retry = false;
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
  std::optional<std::string> provider_options{};
};

struct UserInput {
  std::string text;
  json images = json::array();

  UserInput() = default;
  UserInput(std::string value) : text(std::move(value)) {}
  UserInput(const char* value) : text(value) {}
  UserInput(std::string value, json attachments)
      : text(std::move(value)), images(std::move(attachments)) {}
};

struct ToolResult {
  std::string text;
  json images = json::array();

  ToolResult() = default;
  ToolResult(std::string value) : text(std::move(value)) {}
  ToolResult(const char* value) : text(value) {}
};

struct Tool {
  std::string name;
  std::string description;
  json parameters;
  std::function<ToolResult(const json&)> run;
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
  std::optional<std::string> provider_options;
  std::vector<ToolCall> tool_calls;
  Usage usage;
};

} // namespace niminal
