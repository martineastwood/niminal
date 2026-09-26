#pragma once

#include <niminal/types.hpp>

#include <cail/language_model.hpp>

#include <functional>
#include <map>
#include <optional>
#include <string>

namespace niminal {

struct ProviderResponse {
  int status = 0;
  std::string body;
  std::map<std::string, std::string> headers;
  int duration_ms = 0;
};

struct ChatRequest {
  cail::LanguageModel model;
  json messages;
  json tools;
  int max_tokens = 0;
  json extra = json::object();
  std::string conversation_id;
  std::optional<bool> stream_usage;
  bool apply_cache = false;
  std::function<void(const StreamEvent&)> on_event;
  Cancellation* cancel = nullptr;
  std::function<void(std::map<std::string, std::string>&)> before_provider_headers;
  std::function<void(json&)> before_provider_request;
  std::function<void(const ProviderResponse&)> after_provider_response;
};

ChatResult stream_chat(const ChatRequest& request);
std::string complete_chat(const ChatRequest& request);
std::string format_usage_line(const Usage& usage);

} // namespace niminal
