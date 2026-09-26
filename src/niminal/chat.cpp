#include <niminal/chat.hpp>

#include <cail/anthropic.hpp>
#include <cail/chat_completions.hpp>
#include <cail/foundry.hpp>
#include <cail/gemini.hpp>
#include <cail/hyper.hpp>
#include <cail/local.hpp>
#include <cail/mistral.hpp>
#include <cail/ollama_cloud.hpp>
#include <cail/openai.hpp>
#include <cail/opencode.hpp>
#include <cail/openrouter.hpp>
#include <cail/schema.hpp>
#include <niminal/http.hpp>
#include <niminal/text.hpp>

#include <algorithm>
#include <chrono>
#include <cstdio>
#include <map>
#include <optional>
#include <stop_token>
#include <string_view>
#include <thread>
#include <utility>
#include <vector>

namespace niminal {
namespace {

void require_request(const ChatRequest& request) {
  if (request.api_key.empty() && request.requires_api_key && request.provider != "local") {
    throw Error("missing API key (set " +
                (request.key_hint.empty() ? std::string("OPENROUTER_API_KEY") : request.key_hint) +
                ")");
  }
  if (request.model.empty()) {
    throw Error("missing model");
  }
  if (request.provider == "foundry" && request.api_url.empty()) {
    throw Error("missing Foundry API URL (configure the model in ~/.niminal/models.json)");
  }
}

std::optional<std::string> pick_options(const json& source,
                                        std::initializer_list<const char*> keys) {
  json options = json::object();
  for (const auto* key : keys) {
    if (source.contains(key)) {
      options[key] = source[key];
    }
  }
  return options.empty() ? std::nullopt : std::optional<std::string>{options.dump()};
}

void append_image(cail::Message& message, const json& source) {
  const auto data = source.value("data", "");
  const auto mime = source.value("mime_type", "");
  if (data.empty() || mime.empty()) {
    throw Error("image history requires non-empty base64 data and a MIME type");
  }
  auto bytes = base64_decode(data);
  if (!bytes) {
    throw Error("invalid base64 image data");
  }
  message.content.emplace_back(cail::ImagePart{
      .bytes = std::move(*bytes),
      .mime_type = mime,
      .provider_options = pick_options(source, {"cache_control"}),
  });
}

void append_content(cail::Message& message, const json& content) {
  if (content.is_string()) {
    message.content.emplace_back(cail::TextPart{.text = content.get<std::string>()});
    return;
  }
  if (!content.is_array()) {
    return;
  }
  for (const auto& part : content) {
    if (!part.is_object()) {
      continue;
    }
    if (part.value("type", "") == "image") {
      append_image(message, part);
    } else if (part.value("type", "") == "text" || part.contains("text")) {
      message.content.emplace_back(cail::TextPart{
          .text = part.value("text", ""),
          .provider_options = pick_options(part, {"cache_control"}),
      });
    }
  }
}

void mark_cache(cail::ContentPart& part) {
  constexpr auto cache = R"({"cache_control":{"type":"ephemeral"}})";
  if (auto* text = std::get_if<cail::TextPart>(&part)) {
    text->provider_options = cache;
  } else if (auto* image = std::get_if<cail::ImagePart>(&part)) {
    image->provider_options = cache;
  }
}

void apply_cache(cail::GenerationRequest& out) {
  constexpr auto cache = R"({"cache_control":{"type":"ephemeral"}})";
  for (auto it = out.messages.rbegin(); it != out.messages.rend(); ++it) {
    if (it->role == cail::MessageRole::system && !it->content.empty()) {
      mark_cache(it->content.back());
      break;
    }
  }
  for (auto it = out.messages.rbegin(); it != out.messages.rend(); ++it) {
    if (it->role == cail::MessageRole::tool) {
      it->provider_options = cache;
      break;
    }
    if (!it->content.empty()) {
      mark_cache(it->content.back());
      break;
    }
  }
  if (!out.tools.empty()) {
    out.tools.back().provider_options = cache;
  }
}

cail::MessageRole role_of(std::string_view role) {
  if (role == "system")
    return cail::MessageRole::system;
  if (role == "developer")
    return cail::MessageRole::developer;
  if (role == "assistant")
    return cail::MessageRole::assistant;
  if (role == "tool")
    return cail::MessageRole::tool;
  return cail::MessageRole::user;
}

cail::GenerationRequest make_request(const ChatRequest& request, bool streaming) {
  cail::GenerationRequest out;
  out.session_id = request.conversation_id;
  out.max_output_tokens =
      request.max_tokens > 0
          ? std::optional<std::size_t>{static_cast<std::size_t>(request.max_tokens)}
          : (!streaming ? std::optional<std::size_t>{4096} : std::nullopt);
  out.stream_usage = request.stream_usage;
  if (request.max_tokens <= 0 &&
      (request.provider == "anthropic" || request.model_sdk == "@ai-sdk/anthropic")) {
    out.max_output_tokens = 16384;
  }

  if (request.messages.is_array()) {
    for (const auto& source : request.messages) {
      if (!source.is_object()) {
        continue;
      }
      const auto role = source.value("role", "user");
      cail::Message message{.role = role_of(role)};
      if (message.role == cail::MessageRole::tool) {
        message.tool_call_id = source.value("tool_call_id", "");
      }
      append_content(message, source.value("content", json{}));
      if (message.role == cail::MessageRole::tool && source.contains("images") &&
          source["images"].is_array()) {
        for (const auto& image : source["images"]) {
          if (image.is_object()) {
            append_image(message, image);
          }
        }
      }
      message.provider_options =
          request.provider == "mistral"
              ? pick_options(source, {"cache_control"})
              : pick_options(source, {"reasoning_content", "reasoning_details", "cache_control"});
      if (source.contains("tool_calls") && source["tool_calls"].is_array()) {
        for (const auto& call : source["tool_calls"]) {
          if (!call.is_object()) {
            continue;
          }
          const auto function = call.value("function", json::object());
          message.tool_calls.push_back(cail::ToolCall{
              .id = call.value("id", ""),
              .name = function.value("name", ""),
              .arguments = function.value("arguments", "{}"),
              .provider_options = pick_options(call, {"thought_signature"}),
          });
        }
      }
      out.messages.push_back(std::move(message));
    }
  }

  if (request.tools.is_array()) {
    for (const auto& source : request.tools) {
      if (!source.is_object()) {
        continue;
      }
      const auto function = source.value("function", source);
      auto schema = cail::schema_from_json(function.value("parameters", json::object()).dump());
      if (!schema) {
        throw Error(schema.error().message);
      }
      out.tools.push_back(cail::ToolDefinition{
          .name = function.value("name", ""),
          .description = function.value("description", ""),
          .parameters = std::move(*schema),
      });
    }
  }

  if (request.apply_cache) {
    apply_cache(out);
  }

  json provider_options = request.extra.is_object() ? request.extra : json::object();
  if (!request.conversation_id.empty()) {
    if (request.session_routing && !provider_options.contains("session_id")) {
      provider_options["session_id"] = request.conversation_id;
    }
    if ((request.prompt_cache_key || request.session_routing) &&
        !provider_options.contains("prompt_cache_key")) {
      provider_options["prompt_cache_key"] = request.conversation_id;
    }
  }
  if (!provider_options.empty()) {
    out.provider_options = provider_options.dump();
  }
  return out;
}

[[noreturn]] void throw_provider_error(const cail::Error& error) {
  if (error.code == cail::ErrorCode::cancelled) {
    throw Cancelled{};
  }
  if (error.http_status != 0) {
    throw Error("http " + std::to_string(error.http_status) + ": " + error.message);
  }
  throw Error(error.message);
}

ChatResult convert_response(const cail::GenerationResponse& response) {
  ChatResult result;
  result.text = response.text;
  result.reasoning_content = response.reasoning;
  if (response.provider_options) {
    const auto options = json::parse(*response.provider_options, nullptr, false);
    if (options.is_object() && options.contains("reasoning_details")) {
      result.reasoning_details = options["reasoning_details"];
    }
  }
  if (response.status == cail::GenerationStatus::incomplete) {
    result.finish_reason = "length";
  } else if (response.status == cail::GenerationStatus::refused) {
    result.finish_reason = "stop";
  } else {
    result.finish_reason = response.tool_calls.empty() ? "stop" : "tool_calls";
  }
  for (const auto& call : response.tool_calls) {
    result.tool_calls.push_back(
        ToolCall{.id = call.id, .name = call.name, .arguments = call.arguments});
    if (call.provider_options) {
      const auto options = json::parse(*call.provider_options, nullptr, false);
      if (options.is_object())
        result.tool_calls.back().thought_signature = options.value("thought_signature", "");
    }
  }
  if (response.usage) {
    result.usage.input_tokens = static_cast<int>(response.usage->input_tokens);
    result.usage.output_tokens = static_cast<int>(response.usage->output_tokens);
    result.usage.cache_read_tokens =
        response.usage->cache_read_tokens
            ? static_cast<int>(*response.usage->cache_read_tokens)
            : 0;
    result.usage.cache_write_tokens =
        response.usage->cache_write_tokens
            ? static_cast<int>(*response.usage->cache_write_tokens)
            : 0;
    result.usage.cache_reported = response.usage->cache_read_tokens.has_value() ||
                                  response.usage->cache_write_tokens.has_value();
  }
  return result;
}

void add_request_hooks(cail::GenerationRequest& generation, const ChatRequest& request) {
  if (request.before_provider_request || request.before_provider_headers ||
      !request.extra_headers.empty()) {
    generation.before_request = [&request](cail::HttpRequest& http) {
      if (request.before_provider_request) {
        auto body = json::parse(http.body);
        request.before_provider_request(body);
        http.body = body.dump();
      }
      if (!request.before_provider_headers && request.extra_headers.empty()) {
        return;
      }
      std::map<std::string, std::string> headers;
      for (const auto& header : http.headers) {
        headers[header.name] = header.value;
      }
      for (const auto& [name, value] : request.extra_headers) {
        headers[name] = value;
      }
      if (request.before_provider_headers) {
        request.before_provider_headers(headers);
      }
      http.headers.clear();
      http.headers.reserve(headers.size());
      for (const auto& [name, value] : headers) {
        http.headers.push_back(cail::HttpHeader{.name = name, .value = value});
      }
    };
  }
  if (request.after_provider_response) {
    const auto started = std::chrono::steady_clock::now();
    generation.after_response = [&request, started](const cail::HttpResponse& response) {
      HttpResponse converted;
      converted.status = response.status_code;
      converted.body = response.body;
      for (const auto& header : response.headers) {
        converted.headers[header.name] = header.value;
      }
      converted.duration_ms = static_cast<int>(
          std::chrono::duration_cast<std::chrono::milliseconds>(
              std::chrono::steady_clock::now() - started)
              .count());
      request.after_provider_response(converted);
    };
  }
}

std::string api_base_url(std::string url) {
  for (const auto suffix : {std::string_view{"/chat/completions"}, std::string_view{"/responses"},
                           std::string_view{"/messages"}}) {
    if (url.size() >= suffix.size() &&
        url.compare(url.size() - suffix.size(), suffix.size(), suffix) == 0) {
      url.resize(url.size() - suffix.size());
      break;
    }
  }
  while (!url.empty() && url.back() == '/') {
    url.pop_back();
  }
  return url;
}

cail::LanguageModel make_model(const ChatRequest& request) {
  if (request.provider == "foundry") {
    return cail::create_foundry({.api_key = request.api_key})(cail::FoundryDeployment{
        .endpoint = request.api_url,
        .deployment = request.model,
    });
  }
  if (request.provider == "anthropic") {
    const auto base = api_base_url(request.api_url);
    return cail::create_anthropic(cail::AnthropicSettings{
        .api_key = request.api_key,
        .base_url = base.empty() ? "https://api.anthropic.com/v1" : base,
    })(request.model);
  }
  if (request.provider == "google") {
    auto base = api_base_url(request.api_url);
    if (const auto pos = base.find("/openai"); pos != std::string::npos) base.resize(pos);
    auto model = request.model;
    if (model.starts_with("models/")) model.erase(0, 7);
    return cail::create_gemini(cail::GeminiSettings{
        .api_key = request.api_key,
        .base_url = base.empty() ? "https://generativelanguage.googleapis.com/v1beta" : base,
    })(model);
  }
  if (request.provider == "opencode" || request.provider == "opencodezen") {
    const auto family = [&] {
      if (request.model_sdk == "@ai-sdk/openai-compatible")
        return cail::OpenCodeApiFamily::chat_completions;
      if (request.model_sdk == "@ai-sdk/openai") return cail::OpenCodeApiFamily::responses;
      if (request.model_sdk == "@ai-sdk/anthropic")
        return cail::OpenCodeApiFamily::anthropic_messages;
      if (request.model_sdk == "@ai-sdk/google") return cail::OpenCodeApiFamily::gemini;
      throw Error("Unsupported OpenCode model SDK: " + request.model_sdk);
    }();
    return cail::create_opencode(cail::OpenCodeSettings{
        .api_key = request.api_key,
        .service = request.provider == "opencode" ? cail::OpenCodeService::go
                                                  : cail::OpenCodeService::zen,
        .base_url = api_base_url(request.api_url),
    })(request.model, family);
  }
  if (request.provider == "hyper") {
    return cail::create_hyper(cail::HyperSettings{
        .api_key = request.api_key,
        .endpoint = request.api_url,
    })(request.model);
  }
  if (request.provider == "ollama") {
    return cail::create_ollama_cloud(cail::OllamaCloudSettings{
        .api_key = request.api_key,
        .endpoint = request.api_url,
    })(request.model);
  }
  if (request.provider == "local") {
    return cail::create_local(cail::LocalSettings{
        .api_key = request.api_key,
        .endpoint = request.api_url,
    })(request.model);
  }
  if (request.provider == "openai") {
    return cail::create_openai(cail::OpenAIProviderSettings{
        .api_key = request.api_key,
        .base_url = request.api_url.empty() ? "https://api.openai.com/v1"
                                          : api_base_url(request.api_url),
    })(request.model);
  }
  if (request.provider == "mistral") {
    return cail::create_mistral(cail::MistralSettings{
        .api_key = request.api_key,
        .endpoint = request.api_url,
    })(request.model);
  }
  if (request.provider == "openrouter") {
    return cail::create_openrouter(cail::OpenRouterSettings{
        .api_key = request.api_key,
        .endpoint = request.api_url,
    })(request.model);
  }
  return cail::create_chat_completions({
      .endpoint = request.api_url,
      .api_key = request.api_key,
  })(request.model);
}

ChatResult run_chat(const ChatRequest& request, bool streaming) {
  require_request(request);
  auto generation = make_request(request, streaming);
  add_request_hooks(generation, request);

  std::stop_source stop_source;
  if (request.cancel != nullptr && request.cancel->load()) {
    stop_source.request_stop();
  }
  std::jthread cancellation_watcher;
  if (request.cancel != nullptr) {
    cancellation_watcher =
        std::jthread([cancel = request.cancel, &stop_source](std::stop_token stop) {
          while (!stop.stop_requested()) {
            if (cancel->load()) {
              stop_source.request_stop();
              return;
            }
            std::this_thread::sleep_for(std::chrono::milliseconds(20));
          }
        });
  }

  auto model = make_model(request);
  auto response = [&]() -> cail::Result<cail::GenerationResponse> {
    if (!streaming) {
      return model.generate(generation);
    }
    return model.stream(
        generation,
        [&request](const cail::StreamEvent& event) {
          if (!request.on_event) {
            return;
          }
          if (const auto* delta = std::get_if<cail::TextDelta>(&event)) {
            request.on_event(StreamEvent{EventKind::text_delta, delta->text, {}, {}});
          } else if (const auto* refusal = std::get_if<cail::RefusalDelta>(&event)) {
            request.on_event(StreamEvent{EventKind::text_delta, refusal->text, {}, {}});
          } else if (const auto* thinking = std::get_if<cail::ReasoningDelta>(&event)) {
            request.on_event(StreamEvent{EventKind::thinking_delta, thinking->text, {}, {}});
          }
        },
        stop_source.get_token());
  }();
  cancellation_watcher.request_stop();
  if (!response) {
    throw_provider_error(response.error());
  }
  return convert_response(*response);
}

} // namespace

ChatResult stream_chat(const ChatRequest& request) {
  return run_chat(request, true);
}

std::string complete_chat(const ChatRequest& request) {
  return run_chat(request, false).text;
}

static std::string format_tokens(int count) {
  count = std::max(count, 0);
  struct Scale {
    const char* suffix;
    int value;
  };
  const Scale scales[] = {{"M", 1'000'000}, {"k", 1'000}};
  for (const auto& scale : scales) {
    if (count >= scale.value) {
      int tenths = count * 10 / scale.value;
      if (tenths < 100 && tenths % 10 != 0) {
        return std::to_string(tenths / 10) + "." + std::to_string(tenths % 10) + scale.suffix;
      }
      return std::to_string(count / scale.value) + scale.suffix;
    }
  }
  return std::to_string(count);
}

std::string format_usage_line(const Usage& usage) {
  if (usage.input_tokens == 0 && usage.output_tokens == 0) {
    return {};
  }
  std::string out =
      "↑" + format_tokens(usage.input_tokens) + "  ↓" + format_tokens(usage.output_tokens);
  if (usage.cache_write_tokens > 0) {
    out += "  W" + format_tokens(usage.cache_write_tokens);
  }
  if (usage.cache_read_tokens > 0) {
    int denom = usage.input_tokens;
    if (usage.cache_read_tokens + usage.cache_write_tokens > denom) {
      denom = usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens;
    }
    if (denom > 0) {
      double pct = usage.cache_read_tokens * 100.0 / denom;
      char buf[32];
      std::snprintf(buf, sizeof(buf), "  CH%.1f%%", pct);
      out += buf;
    }
  }
  return out;
}

} // namespace niminal
