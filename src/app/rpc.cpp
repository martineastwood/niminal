#include "rpc.hpp"

#include "json_mode.hpp"
#include "queue_mode.hpp"
#include "shutdown.hpp"

#include <niminal/text.hpp>

#include <algorithm>
#include <atomic>
#include <cerrno>
#include <csignal>
#include <deque>
#include <exception>
#include <iostream>
#include <mutex>
#include <poll.h>
#include <stdexcept>
#include <string>
#include <thread>
#include <unistd.h>
#include <utility>
#include <vector>

namespace niminal::app {
namespace {

volatile std::sig_atomic_t rpc_sigint = 0;

void handle_rpc_sigint(int) {
  rpc_sigint = 1;
}

struct QueuedPrompt {
  std::string id;
  std::string prompt;
  std::string mode;
};

bool string_field(const nlohmann::json& object, const char* name, std::string& value) {
  auto it = object.find(name);
  if (it == object.end() || !it->is_string()) {
    return false;
  }
  value = it->get<std::string>();
  return true;
}

} // namespace

namespace {

class RpcRuntimeImpl {
public:
  RpcRuntimeImpl(niminal::Agent& agent, Session& session, Config& config)
      : agent_(agent), session_(session), config_(config), steering_mode_(config.steering_mode),
        follow_up_mode_(config.follow_up_mode) {}

  int run() {
    const auto previous_sigint = std::signal(SIGINT, handle_rpc_sigint);
    configure_agent();
    send(session_event("session_start", session_.id));

    std::string input;
    bool eof = false;
    while (true) {
      poll_active();
      if ((rpc_sigint != 0 || shutdown_requested()) && !shutting_down_) {
        request_shutdown();
      }
      if (shutting_down_ && !active_) {
        break;
      }

      if (!eof) {
        pollfd ready{STDIN_FILENO, POLLIN | POLLHUP, 0};
        int result;
        do {
          result = ::poll(&ready, 1, 50);
        } while (result < 0 && errno == EINTR);
        if (result < 0) {
          request_shutdown();
          continue;
        }
        if (result > 0 && ((ready.revents & (POLLIN | POLLHUP)) != 0)) {
          char buffer[4096];
          const auto count = ::read(STDIN_FILENO, buffer, sizeof(buffer));
          if (count <= 0) {
            eof = true;
          } else {
            input.append(buffer, static_cast<size_t>(count));
          }
        }
      }

      size_t newline;
      while ((newline = input.find('\n')) != std::string::npos) {
        auto line = input.substr(0, newline);
        input.erase(0, newline + 1);
        line = niminal::trim_copy(std::move(line));
        if (!line.empty()) {
          handle_line(line);
        }
      }
      if (eof) {
        auto line = niminal::trim_copy(std::move(input));
        input.clear();
        if (!line.empty()) {
          handle_line(line);
        }
        request_shutdown();
      }
    }

    if (worker_.joinable()) {
      worker_.join();
    }
    send(session_event("session_end", session_.id, !had_failure_));
    rpc_sigint = 0;
    std::signal(SIGINT, previous_sigint);
    return had_failure_ ? 1 : 0;
  }

private:
  niminal::Agent& agent_;
  Session& session_;
  Config& config_;
  std::mutex queue_mutex_;
  std::mutex output_mutex_;
  std::deque<QueuedPrompt> steering_queue_;
  std::deque<QueuedPrompt> follow_up_queue_;
  std::string steering_mode_;
  std::string follow_up_mode_;
  std::thread worker_;
  niminal::Cancellation cancel_;
  std::atomic<bool> worker_done_{false};
  bool active_ = false;
  bool shutting_down_ = false;
  bool had_failure_ = false;
  bool saw_error_event_ = false;
  int active_step_ = -1;

  void configure_agent() {
    agent_.take_steering = [this] { return take_queue(true); };
    agent_.take_follow_up = [this] { return take_queue(false); };
    agent_.on_event = [this](const niminal::StreamEvent& event) {
      if (event.kind == niminal::EventKind::error) {
        saw_error_event_ = true;
        if (event.text != "interrupted" && event.text != "Interrupted") {
          had_failure_ = true;
        }
      }
      if (event.kind == niminal::EventKind::step_start) {
        active_step_ = event.step;
      }
      if (event.kind == niminal::EventKind::run_start) {
        send(message_event(event.session_id, event.turn_id, "user", event.text));
      }
      send(json_event(event));
    };
    agent_.cancel = &cancel_;
  }

  void send(const nlohmann::json& event) {
    if (event.is_null()) {
      return;
    }
    std::lock_guard lock(output_mutex_);
    std::cout << event.dump() << '\n' << std::flush;
  }

  int queue_depth_locked() const {
    return static_cast<int>(steering_queue_.size() + follow_up_queue_.size());
  }

  std::vector<niminal::UserInput> take_queue(bool steering) {
    std::vector<QueuedPrompt> taken;
    {
      std::lock_guard lock(queue_mutex_);
      auto& queue = steering ? steering_queue_ : follow_up_queue_;
      const auto& mode = steering ? steering_mode_ : follow_up_mode_;
      const size_t count = mode == "all" ? queue.size() : std::min<size_t>(1, queue.size());
      for (size_t i = 0; i < count; ++i) {
        taken.push_back(std::move(queue.front()));
        queue.pop_front();
      }
      for (size_t i = 0; i < taken.size(); ++i) {
        const int depth = queue_depth_locked() + static_cast<int>(taken.size() - i - 1);
        send(queue_event(session_.id, "dequeue", depth, {}, taken[i].id, taken[i].mode));
      }
    }
    std::vector<niminal::UserInput> prompts;
    prompts.reserve(taken.size());
    for (auto& item : taken) {
      prompts.push_back(std::move(item.prompt));
    }
    return prompts;
  }

  void enqueue(QueuedPrompt item) {
    std::lock_guard lock(queue_mutex_);
    auto& queue = item.mode == "steer" ? steering_queue_ : follow_up_queue_;
    queue.push_back(std::move(item));
  }

  std::pair<std::vector<std::string>, std::vector<std::string>> clear_queue() {
    std::vector<std::string> steering;
    std::vector<std::string> follow_up;
    {
      std::lock_guard lock(queue_mutex_);
      for (const auto& item : steering_queue_) {
        steering.push_back(item.prompt);
      }
      for (const auto& item : follow_up_queue_) {
        follow_up.push_back(item.prompt);
      }
      steering_queue_.clear();
      follow_up_queue_.clear();
      if (!steering.empty() || !follow_up.empty()) {
        send(queue_event(session_.id, "clear", 0));
      }
    }
    return {std::move(steering), std::move(follow_up)};
  }

  bool busy() const { return active_; }

  void start_prompt(const std::string& prompt) {
    if (worker_.joinable()) {
      worker_.join();
    }
    cancel_.clear();
    worker_done_.store(false);
    active_step_ = -1;
    saw_error_event_ = false;
    agent_.run_id = session_.id + ":turn:" + std::to_string(session_.events.size());
    active_ = true;
    worker_ = std::thread([this, prompt] { run_prompt(prompt); });
  }

  void run_prompt(const std::string& prompt) {
    try {
      agent_.run(prompt);
    } catch (const std::exception& error) {
      had_failure_ = true;
      if (!saw_error_event_) {
        niminal::StreamEvent event{niminal::EventKind::error, error.what(), {}, {}};
        event.run_id = agent_.run_id;
        event.session_id = session_.id;
        event.turn_id = agent_.run_id;
        event.step = active_step_;
        send(json_event(event));
      }
    }
    worker_done_.store(true);
  }

  void start_queued_prompt() {
    QueuedPrompt item;
    {
      std::lock_guard lock(queue_mutex_);
      if (!steering_queue_.empty()) {
        item = std::move(steering_queue_.front());
        steering_queue_.pop_front();
      } else if (!follow_up_queue_.empty()) {
        item = std::move(follow_up_queue_.front());
        follow_up_queue_.pop_front();
      } else {
        return;
      }
      send(queue_event(session_.id, "dequeue", queue_depth_locked(), {}, item.id, item.mode));
    }
    start_prompt(item.prompt);
  }

  void poll_active() {
    if (!active_ || !worker_done_.load()) {
      return;
    }
    worker_.join();
    active_ = false;
    cancel_.clear();
    if (!shutting_down_) {
      start_queued_prompt();
    }
  }

  void request_shutdown() {
    if (shutting_down_) {
      return;
    }
    shutting_down_ = true;
    clear_queue();
    if (active_) {
      cancel_.request();
    }
  }

  void handle_line(const std::string& line) {
    try {
      handle_command(nlohmann::json::parse(line));
    } catch (const std::exception& error) {
      send(rpc_response_event("", false, {}, "Invalid JSON: " + std::string(error.what())));
    }
  }

  void handle_command(const nlohmann::json& command) {
    if (!command.is_object()) {
      send(rpc_response_event("", false, {}, "Command must be a JSON object."));
      return;
    }
    std::string id;
    std::string type;
    if (!string_field(command, "id", id) || !string_field(command, "type", type)) {
      send(rpc_response_event("", false, {}, "Command requires string id and type fields."));
      return;
    }

    if (type == "prompt") {
      std::string message;
      if (!string_field(command, "message", message) || niminal::trim_copy(message).empty()) {
        send(rpc_response_event(id, false, {}, "prompt requires message."));
      } else if (shutting_down_) {
        send(rpc_response_event(id, false, {}, "RPC is shutting down."));
      } else if (!busy()) {
        send(rpc_response_event(id, true, "started"));
        start_prompt(message);
      } else {
        std::string behavior;
        if (!string_field(command, "streamingBehavior", behavior) ||
            (behavior != "steer" && behavior != "followUp")) {
          send(rpc_response_event(
              id, false, {}, "prompt requires streamingBehavior: steer or followUp while busy."));
        } else {
          const std::string mode = behavior == "steer" ? "steer" : "follow_up";
          enqueue({id, message, mode});
          send(rpc_response_event(id, true, "queued"));
          send(queue_event(session_.id, "enqueue", queue_depth(), message, id, mode));
        }
      }
      return;
    }

    if (type == "steer" || type == "follow_up") {
      std::string message;
      if (!string_field(command, "message", message) || niminal::trim_copy(message).empty()) {
        send(rpc_response_event(id, false, {}, type + " requires message."));
      } else if (shutting_down_) {
        send(rpc_response_event(id, false, {}, "RPC is shutting down."));
      } else if (!busy()) {
        send(rpc_response_event(id, false, {}, "Agent is idle; use prompt."));
      } else {
        const std::string mode = type == "steer" ? "steer" : "follow_up";
        enqueue({id, message, mode});
        send(rpc_response_event(id, true, "queued"));
        send(queue_event(session_.id, "enqueue", queue_depth(), message, id, mode));
      }
      return;
    }

    if (type == "interrupt") {
      if (busy()) {
        cancel_.request();
      }
      send(rpc_response_event(id, true, busy() ? "interrupting" : "idle"));
      return;
    }

    if (type == "get_state") {
      nlohmann::json response = rpc_response_event(id, true);
      response["session_id"] = session_.id;
      response["busy"] = busy();
      response["queued"] = queue_depth() > 0;
      {
        std::lock_guard lock(queue_mutex_);
        response["steering"] = steering_queue_.size();
        response["follow_up"] = follow_up_queue_.size();
        response["steering_mode"] = steering_mode_;
        response["follow_up_mode"] = follow_up_mode_;
      }
      response["mode"] = "act";
      send(response);
      return;
    }

    if (type == "clear_queue") {
      auto removed = clear_queue();
      nlohmann::json response = rpc_response_event(id, true);
      response["steering"] = std::move(removed.first);
      response["follow_up"] = std::move(removed.second);
      send(response);
      return;
    }

    if (type == "set_steering_mode" || type == "set_follow_up_mode") {
      std::string mode;
      if (!string_field(command, "mode", mode) || !valid_queue_mode(mode)) {
        send(rpc_response_event(id, false, {}, "mode must be all or one-at-a-time."));
        return;
      }
      try {
        {
          std::lock_guard lock(queue_mutex_);
          if (type == "set_steering_mode") {
            config_.steering_mode = mode;
            steering_mode_ = mode;
          } else {
            config_.follow_up_mode = mode;
            follow_up_mode_ = mode;
          }
        }
        save_config(config_);
        send(rpc_response_event(id, true));
      } catch (const std::exception& error) {
        send(rpc_response_event(id, false, {}, error.what()));
      }
      return;
    }

    if (type == "shutdown") {
      const bool was_busy = busy();
      request_shutdown();
      send(rpc_response_event(id, true, was_busy ? "stopping" : "stopped"));
      return;
    }

    send(rpc_response_event(id, false, {}, "Unknown RPC command: " + type));
  }

  int queue_depth() {
    std::lock_guard lock(queue_mutex_);
    return queue_depth_locked();
  }
};

} // namespace

int run_rpc(niminal::Agent& agent, Session& session, Config& config) {
  return RpcRuntimeImpl(agent, session, config).run();
}

} // namespace niminal::app
