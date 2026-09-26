#include <niminal/chat.hpp>

#include <cail/http.hpp>
#include <cail/json.hpp>
#include <cail/schema.hpp>
#include <niminal/text.hpp>

#include <algorithm>
#include <chrono>
#include <cstdio>
#include <map>
#include <optional>
#include <string_view>
#include <utility>
#include <vector>

namespace niminal {
namespace {

std::optional<std::string> json_options(const json& value) {
  if (value.is_string()) {
    const auto text = value.get<std::string>();
    return text.empty() ? std::nullopt : std::optional<std::string>{text};
  }
  if (value.is_object() && !value.empty()) {
    return value.dump();
  }
  return std::nullopt;
}

std::optional<std::string> provider_options_of(const json& source) {
  if (!source.is_object() || !source.contains("provider_options")) {
    return std::nullopt;
  }
  return json_options(source["provider_options"]);
}

std::optional<std::string> merge_options(std::optional<std::string> existing,
                                         std::string_view extra) {
  if (!existing || existing->empty()) {
    return std::string{extra};
  }
  auto merged = cail::merge_json_objects(*existing, extra);
  if (!merged) {
    return existing;
  }
  return std::move(*merged);
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
      .provider_options = provider_options_of(source),
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
          .provider_options = provider_options_of(part),
      });
    }
  }
}

void mark_cache(cail::ContentPart& part) {
  constexpr auto cache = R"({"cache_control":{"type":"ephemeral"}})";
  if (auto* text = std::get_if<cail::TextPart>(&part)) {
    text->provider_options = merge_options(std::move(text->provider_options), cache);
  } else if (auto* image = std::get_if<cail::ImagePart>(&part)) {
    image->provider_options = merge_options(std::move(image->provider_options), cache);
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
      it->provider_options = merge_options(std::move(it->provider_options), cache);
      break;
    }
    if (!it->content.empty()) {
      mark_cache(it->content.back());
      break;
    }
  }
  if (!out.tools.empty()) {
    out.tools.back().provider_options =
        merge_options(std::move(out.tools.back().provider_options), cache);
  }
}

cail::MessageRole role_of(std::string_view role) {
  if (role == "system") {
    return cail::MessageRole::system;
  }
  if (role == "developer") {
    return cail::MessageRole::developer;
  }
  if (role == "assistant") {
    return cail::MessageRole::assistant;
  }
  if (role == "tool") {
    return cail::MessageRole::tool;
  }
  return cail::MessageRole::user;
}

cail::GenerationRequest make_request(const ChatRequest& request) {
  cail::GenerationRequest out;
  out.session_id = request.conversation_id;
  if (request.max_tokens > 0) {
    out.max_output_tokens = static_cast<std::size_t>(request.max_tokens);
  }
  out.stream_usage = request.stream_usage;

  if (request.messages.is_array()) {
    for (const auto& source : request.messages) {
      if (!source.is_object()) {
        continue;
      }
      cail::Message message{.role = role_of(source.value("role", "user"))};
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
      message.provider_options = provider_options_of(source);
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
              .provider_options = provider_options_of(call),
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

  if (request.extra.is_object() && !request.extra.empty()) {
    out.provider_options = request.extra.dump();
  }
  return out;
}

[[noreturn]] void throw_provider_error(const cail::Error& error) {
  if (error.code == cail::ErrorCode::cancelled) {
    throw Cancelled{};
  }
  const bool transport = error.code == cail::ErrorCode::transport;
  if (error.http_status != 0) {
    throw Error("http " + std::to_string(error.http_status) + ": " + error.message,
                error.http_status, transport);
  }
  throw Error(error.message, 0, transport);
}

ChatResult convert_response(const cail::GenerationResponse& response) {
  ChatResult result;
  result.text = response.text;
  result.provider_options = response.provider_options;
  for (const auto& call : response.tool_calls) {
    result.tool_calls.push_back(ToolCall{
        .id = call.id,
        .name = call.name,
        .arguments = call.arguments,
        .provider_options = call.provider_options,
    });
  }
  if (response.usage) {
    result.usage.input_tokens = static_cast<int>(response.usage->input_tokens);
    result.usage.output_tokens = static_cast<int>(response.usage->output_tokens);
    result.usage.cache_read_tokens = response.usage->cache_read_tokens
                                         ? static_cast<int>(*response.usage->cache_read_tokens)
                                         : 0;
    result.usage.cache_write_tokens = response.usage->cache_write_tokens
                                          ? static_cast<int>(*response.usage->cache_write_tokens)
                                          : 0;
    result.usage.cache_reported = response.usage->cache_read_tokens.has_value() ||
                                  response.usage->cache_write_tokens.has_value();
  }
  return result;
}

void add_request_hooks(cail::GenerationRequest& generation, const ChatRequest& request) {
  if (request.before_provider_request || request.before_provider_headers) {
    generation.before_request = [&request](cail::HttpRequest& http) {
      if (request.before_provider_headers) {
        std::map<std::string, std::string> headers;
        for (const auto& header : http.headers) {
          headers[header.name] = header.value;
        }
        request.before_provider_headers(headers);
        http.headers.clear();
        http.headers.reserve(headers.size());
        for (const auto& [name, value] : headers) {
          http.headers.push_back(cail::HttpHeader{.name = name, .value = value});
        }
      }
      if (!request.before_provider_request) {
        return;
      }
      auto body = json::parse(http.body, nullptr, false);
      if (!body.is_object()) {
        throw Error("provider request body is not a JSON object");
      }
      request.before_provider_request(body);
      http.body = body.dump();
    };
  }
  if (request.after_provider_response) {
    const auto started = std::chrono::steady_clock::now();
    generation.after_response = [&request, started](const cail::HttpResponse& response) {
      ProviderResponse converted;
      converted.status = response.status_code;
      converted.body = response.body;
      for (const auto& header : response.headers) {
        converted.headers[header.name] = header.value;
      }
      converted.duration_ms =
          static_cast<int>(std::chrono::duration_cast<std::chrono::milliseconds>(
                               std::chrono::steady_clock::now() - started)
                               .count());
      request.after_provider_response(converted);
    };
  }
}

} // namespace

namespace {

ChatResult run_chat(const ChatRequest& request, bool streaming) {
  if (!request.model) {
    throw Error("language model is not configured");
  }
  auto generation = make_request(request);
  add_request_hooks(generation, request);
  std::stop_token stop;
  if (request.cancel != nullptr) {
    if (request.cancel->requested()) {
      request.cancel->request();
    }
    stop = request.cancel->token();
  }

  auto response = [&]() -> cail::Result<cail::GenerationResponse> {
    if (!streaming) {
      return request.model.generate(generation);
    }
    return request.model.stream(
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
        stop);
  }();
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
