#pragma once

#include <niminal/types.hpp>

#include <cail/language_model.hpp>

#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <vector>

namespace niminal {

struct ChatRequest;
struct ProviderResponse;

struct Agent {
  std::string system;
  std::string model;
  std::optional<bool> stream_usage;
  bool apply_cache = false;
  cail::LanguageModel language_model;
  int max_steps = 0; // 0 means unlimited.
  std::vector<Tool> tools;
  json messages = json::array();
  json extra = json::object();
  std::function<void(const StreamEvent&)> on_event;
  std::shared_ptr<std::function<void(std::string)>> tool_output =
      std::make_shared<std::function<void(std::string)>>();
  Cancellation* cancel = nullptr;
  std::vector<std::string> system_extra;
  std::function<std::vector<std::string>()> system_extra_loader;
  std::string conversation_id;
  std::function<void(const UserInput&)> persist_user;
  std::function<void(const std::string& text, const std::vector<ToolCall>& calls,
                     const std::string& model, const Usage& usage, const json& provider_options)>
      persist_assistant;
  std::function<void(const std::string& id, const ToolResult& output, bool error)> persist_tool;
  std::function<void()> persist_step;
  std::function<bool(const ToolCall&, const Tool&)> approve_tool;
  std::function<bool(const ToolCall&, json& arguments, std::string& reason)> before_tool;
  std::function<void(const ToolCall&, const json& arguments, std::string& output, bool& is_error)>
      after_tool;
  std::function<void(json& request_messages)> augment_context;
  std::function<void()> turn_start;
  std::function<void(bool interrupted)> turn_end;
  std::function<void(UserInput&)> input_hook;
  std::function<void(const UserInput&, std::string&, json&)> before_agent_start;
  std::function<void(const json&)> persist_extension_message;
  std::function<void(json&)> message_end;
  std::function<void()> agent_settled;
  std::function<void(std::map<std::string, std::string>&)> before_provider_headers;
  std::function<void(json&)> before_provider_request;
  std::function<void(const ProviderResponse&)> after_provider_response;
  std::function<std::vector<UserInput>()> take_steering;
  std::function<std::vector<UserInput>()> take_follow_up;
  std::function<UserInput(UserInput)> prepare_user;
  std::function<void()> before_request;
  std::function<bool()> recover_overflow;
  std::function<ChatResult(const ChatRequest&)> stream_chat_fn;
  std::string run_id;

  json request_messages(const std::string& effective_system) const;
  json request_messages() const { return request_messages(system); }
  void fill_chat(ChatRequest& req) const;
  bool cancelled() const { return cancel != nullptr && cancel->requested(); }
  std::string run(UserInput prompt, bool append_user = true);
};

} // namespace niminal
