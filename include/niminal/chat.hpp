#pragma once

#include <niminal/types.hpp>

#include <atomic>
#include <functional>
#include <map>
#include <string>

namespace niminal {

struct HttpResponse;

struct ChatRequest {
  std::string api_url;
  std::string api_key;
  std::string model;
  std::string provider;
  std::string model_sdk;
  std::string key_hint = "OPENROUTER_API_KEY";
  json messages;
  json tools;
  int max_tokens = 0;
  json extra = json::object();
  std::string conversation_id;
  std::map<std::string, std::string> extra_headers;
  bool session_routing = false;
  bool stream_usage = true;
  bool apply_cache = false;
  bool prompt_cache_key = false;
  std::function<void(const StreamEvent&)> on_event;
  std::atomic<bool>* cancel = nullptr;
  std::function<void(std::map<std::string, std::string>&)> before_provider_headers;
  std::function<void(json&)> before_provider_request;
  std::function<void(const HttpResponse&)> after_provider_response;
  bool requires_api_key = true;
};

ChatResult stream_chat(const ChatRequest& request);
std::string complete_chat(const ChatRequest& request);
std::string format_usage_line(const Usage& usage);

} // namespace niminal
