#include "tool_input.hpp"

#include <algorithm>
#include <niminal/http.hpp>
#include <niminal/openai.hpp>
#include <niminal/text.hpp>

#include <cstdio>
#include <map>
#include <stdexcept>
#include <utility>
#include <vector>

namespace niminal {

namespace {

enum class Wire { Chat, Responses, Anthropic, Google };

Wire wire_of(const ChatRequest& request) {
  if (request.provider == "foundry") {
    return Wire::Responses;
  }
  if (request.provider == "anthropic") {
    return Wire::Anthropic;
  }
  if (request.provider == "google") {
    return Wire::Google;
  }
  return Wire::Chat;
}

void emit(const ChatRequest& request, const StreamEvent& ev) {
  if (request.on_event) {
    request.on_event(ev);
  }
}

void emit_thinking(const ChatRequest& request, const std::string& text) {
  if (!text.empty()) {
    emit(request, StreamEvent{EventKind::thinking_delta, text, {}, {}});
  }
}

void emit_thinking_value(const ChatRequest& request, const json& value) {
  if (value.is_string()) {
    emit_thinking(request, value.get<std::string>());
  } else if (value.is_array()) {
    for (const auto& item : value) {
      emit_thinking_value(request, item);
    }
  } else if (value.is_object()) {
    for (const auto* key : {"text", "content", "reasoning", "reasoning_content"}) {
      if (value.contains(key)) {
        emit_thinking_value(request, value[key]);
      }
    }
  }
}

void require_request(const ChatRequest& request) {
  if (request.api_key.empty() && request.requires_api_key && request.provider != "local") {
    throw Error("missing API key (set " +
                (request.key_hint.empty() ? std::string("OPENROUTER_API_KEY") : request.key_hint) +
                ")");
  }
  if (request.model.empty()) {
    throw Error("missing model");
  }
  if (wire_of(request) == Wire::Responses && request.api_url.empty()) {
    throw Error("missing Foundry API URL (configure the model in ~/.niminal/models.json)");
  }
}

json ephemeral_cache() {
  json cc = json::object();
  cc["type"] = "ephemeral";
  return cc;
}

void mark_last_array_cache(json& node) {
  if (!node.is_array() || node.empty() || !node.back().is_object()) {
    return;
  }
  node.back()["cache_control"] = ephemeral_cache();
}

bool mark_content_cache(json& msg) {
  if (!msg.is_object()) {
    return false;
  }
  if (msg.value("role", "") == "tool") {
    msg["cache_control"] = ephemeral_cache();
    return true;
  }
  if (!msg.contains("content")) {
    return false;
  }
  auto& c = msg["content"];
  if (c.is_array() && !c.empty() && c.back().is_object()) {
    c.back()["cache_control"] = ephemeral_cache();
    return true;
  }
  return false;
}

bool uses_explicit_cache(std::string_view model) {
  const std::string m = lower_copy(std::string(model));
  return m.find("anthropic") != std::string::npos || m.find("claude") != std::string::npos ||
         m.find("gemini") != std::string::npos || m.find("google/") != std::string::npos ||
         m.find("vertex") != std::string::npos;
}

void normalize_message_content(json& msg) {
  if (!msg.is_object()) {
    return;
  }
  if (msg.value("role", "") == "tool") {
    return;
  }
  if (!msg.contains("content") || !msg["content"].is_string()) {
    return;
  }
  json part = json::object();
  part["type"] = "text";
  part["text"] = msg["content"].get<std::string>();
  json arr = json::array();
  arr.push_back(std::move(part));
  msg["content"] = std::move(arr);
}

void normalize_messages(json& messages) {
  if (!messages.is_array()) {
    return;
  }
  for (auto& msg : messages) {
    normalize_message_content(msg);
  }
}

void apply_cache_breakpoints(json& body) {
  if (!body.is_object()) {
    return;
  }
  if (body.contains("tools")) {
    mark_last_array_cache(body["tools"]);
  }
  if (body.contains("system") && body["system"].is_array()) {
    mark_last_array_cache(body["system"]);
  }
  if (body.contains("messages") && body["messages"].is_array()) {
    auto& msgs = body["messages"];
    for (int i = static_cast<int>(msgs.size()) - 1; i >= 0; --i) {
      if (msgs[static_cast<size_t>(i)].is_object() &&
          msgs[static_cast<size_t>(i)].value("role", "") == "system") {
        mark_content_cache(msgs[static_cast<size_t>(i)]);
        break;
      }
    }
    for (int i = static_cast<int>(msgs.size()) - 1; i >= 0; --i) {
      if (msgs[static_cast<size_t>(i)].is_object() &&
          mark_content_cache(msgs[static_cast<size_t>(i)])) {
        break;
      }
    }
  }
}

json text_parts(const json& content) {
  json parts = json::array();
  if (content.is_string()) {
    json part = json::object();
    part["type"] = "text";
    part["text"] = content.get<std::string>();
    parts.push_back(std::move(part));
    return parts;
  }
  if (!content.is_array()) {
    return parts;
  }
  for (const auto& item : content) {
    if (item.is_string()) {
      json part = json::object();
      part["type"] = "text";
      part["text"] = item.get<std::string>();
      parts.push_back(std::move(part));
    } else if (item.is_object() && (item.value("type", "") == "text" || item.contains("text"))) {
      json part = json::object();
      part["type"] = "text";
      part["text"] = item.value("text", "");
      parts.push_back(std::move(part));
    }
  }
  return parts;
}

json openai_image(const json& image) {
  return json{
      {"type", "image_url"},
      {"image_url",
       {{"url", "data:" + image.value("mime_type", "") + ";base64," + image.value("data", "")}}}};
}

json anthropic_image(const json& image) {
  return json{{"type", "image"},
              {"source",
               {{"type", "base64"},
                {"media_type", image.value("mime_type", "")},
                {"data", image.value("data", "")}}}};
}

json google_image(const json& image) {
  return json{{"inlineData",
               {{"mimeType", image.value("mime_type", "")}, {"data", image.value("data", "")}}}};
}

json image_parts(const json& content, json (*convert)(const json&)) {
  json out = json::array();
  if (!content.is_array()) {
    return out;
  }
  for (const auto& part : content) {
    if (part.is_object() && part.value("type", "") == "image") {
      out.push_back(convert(part));
    }
  }
  return out;
}

std::string join_text(const json& content) {
  if (content.is_string()) {
    return content.get<std::string>();
  }
  std::string out;
  for (const auto& part : text_parts(content)) {
    out += part.value("text", "");
  }
  return out;
}

json openai_chat_body(const ChatRequest& request) {
  json payload = json::object();
  payload["model"] = request.model;
  payload["stream"] = request.stream;
  if (request.stream && request.stream_usage) {
    payload["stream_options"] = json{{"include_usage", true}};
  }
  payload["messages"] = json::array();
  json pending_images = json::array();
  auto flush_images = [&] {
    if (!pending_images.empty()) {
      payload["messages"].push_back(json{{"role", "user"}, {"content", pending_images}});
      pending_images = json::array();
    }
  };
  for (const auto& source : request.messages) {
    if (!source.is_object()) {
      continue;
    }
    const auto role = source.value("role", "");
    if (role != "tool") {
      flush_images();
    }
    json msg = source;
    msg.erase("images");
    if (role == "user" && msg.contains("content") && msg["content"].is_array()) {
      json parts = json::array();
      for (const auto& part : msg["content"]) {
        parts.push_back(part.is_object() && part.value("type", "") == "image" ? openai_image(part)
                                                                              : part);
      }
      msg["content"] = std::move(parts);
    }
    payload["messages"].push_back(std::move(msg));
    if (role == "tool" && source.contains("images")) {
      for (const auto& part : image_parts(source["images"], openai_image)) {
        pending_images.push_back(part);
      }
    }
  }
  flush_images();
  if (request.max_tokens > 0) {
    payload["max_tokens"] = request.max_tokens;
  }
  normalize_messages(payload["messages"]);
  if (request.session_routing && !request.conversation_id.empty()) {
    payload["session_id"] = request.conversation_id;
  }
  if ((request.prompt_cache_key || request.session_routing) && !request.conversation_id.empty() &&
      !payload.contains("prompt_cache_key")) {
    payload["prompt_cache_key"] = request.conversation_id;
  }
  if (!request.tools.is_null() && !request.tools.empty()) {
    payload["tools"] = request.tools;
  }
  bool cache = request.apply_cache;
  if (!cache && request.provider.empty()) {
    cache = uses_explicit_cache(request.model);
  }
  if (cache) {
    apply_cache_breakpoints(payload);
  }
  return payload;
}

json responses_image(const json& image) {
  return json{
      {"type", "input_image"},
      {"image_url", "data:" + image.value("mime_type", "") + ";base64," + image.value("data", "")}};
}

json responses_message(const json& source) {
  json message = json::object();
  const auto role = source.value("role", "user");
  message["role"] = role;
  const auto content = source.value("content", json(""));
  bool has_image = false;
  json parts = json::array();
  if (content.is_array()) {
    for (const auto& part : content) {
      if (part.is_object() && part.value("type", "") == "image") {
        parts.push_back(responses_image(part));
        has_image = true;
      } else {
        const auto text = part.is_string()   ? part.get<std::string>()
                          : part.is_object() ? part.value("text", "")
                                             : "";
        if (!text.empty()) {
          parts.push_back(json{{"type", "input_text"}, {"text", text}});
        }
      }
    }
  }
  message["content"] = has_image ? std::move(parts) : json(join_text(content));
  return message;
}

json responses_tools(const json& tools) {
  json out = json::array();
  if (!tools.is_array()) {
    return out;
  }
  for (const auto& tool : tools) {
    if (!tool.is_object()) {
      continue;
    }
    const auto fn =
        tool.contains("function") && tool["function"].is_object() ? tool["function"] : tool;
    json item = json::object();
    item["type"] = "function";
    item["name"] = fn.value("name", tool.value("name", ""));
    item["description"] = fn.value("description", "");
    item["parameters"] = fn.value("parameters", json::object());
    if (fn.contains("strict")) {
      item["strict"] = fn["strict"];
    }
    out.push_back(std::move(item));
  }
  return out;
}

json responses_body(const ChatRequest& request) {
  json payload = json::object();
  payload["model"] = request.model;
  payload["stream"] = request.stream;
  payload["input"] = json::array();
  if (request.messages.is_array()) {
    for (const auto& source : request.messages) {
      if (!source.is_object()) {
        continue;
      }
      const auto role = source.value("role", "user");
      if (role == "tool") {
        payload["input"].push_back(json{{"type", "function_call_output"},
                                        {"call_id", source.value("tool_call_id", "")},
                                        {"output", join_text(source.value("content", json("")))}});
        if (source.contains("images")) {
          json content = image_parts(source["images"], responses_image);
          if (!content.empty()) {
            payload["input"].push_back(json{{"role", "user"}, {"content", std::move(content)}});
          }
        }
        continue;
      }
      const auto text = join_text(source.value("content", json("")));
      if (role != "assistant" || !source.contains("tool_calls") ||
          !source["tool_calls"].is_array()) {
        payload["input"].push_back(responses_message(source));
        continue;
      }
      if (!text.empty()) {
        payload["input"].push_back(responses_message(source));
      }
      for (const auto& call : source["tool_calls"]) {
        if (!call.is_object()) {
          continue;
        }
        const auto fn = call.value("function", json::object());
        payload["input"].push_back(json{{"type", "function_call"},
                                        {"call_id", call.value("id", "")},
                                        {"name", fn.value("name", "")},
                                        {"arguments", fn.value("arguments", "{}")}});
      }
    }
  }
  if (request.max_tokens > 0) {
    payload["max_output_tokens"] = request.max_tokens;
  }
  auto tools = responses_tools(request.tools);
  if (!tools.empty()) {
    payload["tools"] = std::move(tools);
  }
  if ((request.prompt_cache_key || request.session_routing) && !request.conversation_id.empty()) {
    payload["prompt_cache_key"] = request.conversation_id;
  }
  return payload;
}

json anthropic_tools(const json& tools) {
  json out = json::array();
  if (!tools.is_array()) {
    return out;
  }
  for (const auto& tool : tools) {
    if (!tool.is_object()) {
      continue;
    }
    json fn = tool.contains("function") && tool["function"].is_object() ? tool["function"] : tool;
    json item = json::object();
    item["type"] = "custom";
    item["name"] = fn.value("name", tool.value("name", ""));
    item["description"] = fn.value("description", "");
    item["input_schema"] = fn.contains("parameters") ? fn["parameters"] : json::object();
    out.push_back(std::move(item));
  }
  return out;
}

json anthropic_body(const ChatRequest& request) {
  json payload = json::object();
  payload["model"] = request.model;
  payload["max_tokens"] = request.max_tokens > 0 ? request.max_tokens : 16384;
  payload["stream"] = request.stream;
  json system = json::array();
  json messages = json::array();
  json pending_tools = json::array();
  auto flush_tools = [&] {
    if (pending_tools.empty()) {
      return;
    }
    json user = json::object();
    user["role"] = "user";
    user["content"] = std::move(pending_tools);
    messages.push_back(std::move(user));
    pending_tools = json::array();
  };
  if (request.messages.is_array()) {
    for (const auto& msg : request.messages) {
      if (!msg.is_object()) {
        continue;
      }
      auto role = msg.value("role", "");
      if (role == "system") {
        for (auto& part : text_parts(msg.value("content", json("")))) {
          system.push_back(std::move(part));
        }
        continue;
      }
      if (role == "tool") {
        json block = json::object();
        block["type"] = "tool_result";
        block["tool_use_id"] = msg.value("tool_call_id", "");
        json content = text_parts(msg.value("content", json("")));
        if (msg.contains("images")) {
          for (const auto& image : image_parts(msg["images"], anthropic_image)) {
            content.push_back(image);
          }
        }
        block["content"] = std::move(content);
        pending_tools.push_back(std::move(block));
        continue;
      }
      flush_tools();
      json content = text_parts(msg.value("content", json("")));
      if (role == "user") {
        for (const auto& image :
             image_parts(msg.value("content", json::array()), anthropic_image)) {
          content.push_back(image);
        }
      }
      if (role == "assistant" && msg.contains("tool_calls") && msg["tool_calls"].is_array()) {
        for (const auto& call : msg["tool_calls"]) {
          json fn = call.value("function", json::object());
          json use = json::object();
          use["type"] = "tool_use";
          use["id"] = call.value("id", "");
          use["name"] = fn.value("name", "");
          use["input"] = detail::parse_tool_input(fn.value("arguments", ""));
          content.push_back(std::move(use));
        }
      }
      json out = json::object();
      out["role"] = role == "assistant" ? "assistant" : "user";
      out["content"] = std::move(content);
      messages.push_back(std::move(out));
    }
  }
  flush_tools();
  if (!system.empty()) {
    payload["system"] = std::move(system);
  }
  payload["messages"] = std::move(messages);
  auto tools = anthropic_tools(request.tools);
  if (!tools.empty()) {
    payload["tools"] = std::move(tools);
  }
  apply_cache_breakpoints(payload);
  return payload;
}

json google_tools(const json& tools) {
  json decls = json::array();
  if (!tools.is_array()) {
    return decls;
  }
  for (const auto& tool : tools) {
    if (!tool.is_object()) {
      continue;
    }
    json fn = tool.contains("function") && tool["function"].is_object() ? tool["function"] : tool;
    json item = json::object();
    item["name"] = fn.value("name", tool.value("name", ""));
    item["description"] = fn.value("description", "");
    item["parametersJsonSchema"] = fn.contains("parameters") ? fn["parameters"] : json::object();
    decls.push_back(std::move(item));
  }
  return decls;
}

json google_body(const ChatRequest& request) {
  json payload = json::object();
  json system_parts = json::array();
  json contents = json::array();
  json pending = json::array();
  std::map<std::string, std::string> call_names;
  auto flush_user = [&] {
    if (pending.empty()) {
      return;
    }
    json msg = json::object();
    msg["role"] = "user";
    msg["parts"] = std::move(pending);
    contents.push_back(std::move(msg));
    pending = json::array();
  };
  if (request.messages.is_array()) {
    for (const auto& msg : request.messages) {
      if (!msg.is_object()) {
        continue;
      }
      auto role = msg.value("role", "");
      if (role == "system") {
        for (const auto& part : text_parts(msg.value("content", json("")))) {
          json p = json::object();
          p["text"] = part.value("text", "");
          system_parts.push_back(std::move(p));
        }
        continue;
      }
      if (role == "tool") {
        json resp = json::object();
        auto id = msg.value("tool_call_id", "");
        auto name = (call_names.count(id) != 0U) ? call_names[id] : id;
        json fr = json::object();
        fr["name"] = name;
        fr["response"] = json{{"output", join_text(msg.value("content", json("")))}};
        resp["functionResponse"] = std::move(fr);
        pending.push_back(std::move(resp));
        if (msg.contains("images")) {
          for (const auto& image : image_parts(msg["images"], google_image)) {
            pending.push_back(image);
          }
        }
        continue;
      }
      flush_user();
      json parts = json::array();
      auto text = join_text(msg.value("content", json("")));
      if (!text.empty()) {
        json p = json::object();
        p["text"] = text;
        parts.push_back(std::move(p));
      }
      if (role == "user") {
        for (const auto& image : image_parts(msg.value("content", json::array()), google_image)) {
          parts.push_back(image);
        }
      }
      if (role == "assistant" && msg.contains("tool_calls") && msg["tool_calls"].is_array()) {
        for (const auto& call : msg["tool_calls"]) {
          json fn = call.value("function", json::object());
          auto id = call.value("id", "");
          auto name = fn.value("name", "");
          if (!id.empty()) {
            call_names[id] = name;
          }
          json fc = json::object();
          fc["name"] = name;
          fc["args"] = detail::parse_tool_input(fn.value("arguments", ""));
          json part = json::object();
          part["functionCall"] = std::move(fc);
          parts.push_back(std::move(part));
        }
      }
      if (parts.empty()) {
        continue;
      }
      json out = json::object();
      out["role"] = role == "assistant" ? "model" : "user";
      out["parts"] = std::move(parts);
      contents.push_back(std::move(out));
    }
  }
  flush_user();
  if (!system_parts.empty()) {
    json sys = json::object();
    sys["parts"] = std::move(system_parts);
    payload["systemInstruction"] = std::move(sys);
  }
  payload["contents"] = std::move(contents);
  if (request.max_tokens > 0) {
    payload["generationConfig"] = json{{"maxOutputTokens", request.max_tokens}};
  }
  auto decls = google_tools(request.tools);
  if (!decls.empty()) {
    json tool = json::object();
    tool["functionDeclarations"] = std::move(decls);
    payload["tools"] = json::array({std::move(tool)});
  }
  return payload;
}

void apply_google_thinking(json& payload, const ChatRequest& request, json& extra) {
  if (!extra.contains("reasoning_effort") || !extra["reasoning_effort"].is_string()) {
    return;
  }
  auto effort = extra["reasoning_effort"].get<std::string>();
  extra.erase("reasoning_effort");
  if (!payload.contains("generationConfig") || !payload["generationConfig"].is_object()) {
    payload["generationConfig"] = json::object();
  }
  auto& config = payload["generationConfig"];
  if (request.model.rfind("gemini-2.5", 0) == 0) {
    int budget = -1;
    if (effort == "none") {
      budget = 0;
    } else if (effort == "minimal" || effort == "low") {
      budget = 1024;
    } else if (effort == "medium") {
      budget = 8192;
    } else if (effort == "high") {
      budget = 24576;
    }
    config["thinkingConfig"] = json{{"thinkingBudget", budget}};
  } else {
    auto level = effort;
    for (char& c : level) {
      if (c >= 'a' && c <= 'z') {
        c = static_cast<char>(c - 'a' + 'A');
      }
    }
    config["thinkingConfig"] = json{{"thinkingLevel", level}};
  }
}

void merge_extra(json& payload, const ChatRequest& request) {
  if (!request.extra.is_object() || request.extra.empty()) {
    return;
  }
  json extra = request.extra;
  if (wire_of(request) == Wire::Google) {
    apply_google_thinking(payload, request, extra);
  }
  for (auto& [key, value] : extra.items()) {
    if (value.is_object() && payload.contains(key) && payload[key].is_object()) {
      payload[key].update(value);
    } else {
      payload[key] = value;
    }
  }
}

std::string strip_suffix(std::string url, std::string_view suffix) {
  if (url.size() >= suffix.size() &&
      url.compare(url.size() - suffix.size(), suffix.size(), suffix.data()) == 0) {
    url.resize(url.size() - suffix.size());
  }
  return url;
}

std::string google_base(std::string url) {
  auto openai = url.find("/openai/");
  if (openai != std::string::npos) {
    url.resize(openai);
  }
  url = strip_suffix(std::move(url), "/chat/completions");
  url = strip_suffix(std::move(url), "/responses");
  url = strip_suffix(std::move(url), "/messages");
  while (!url.empty() && url.back() == '/') {
    url.pop_back();
  }
  return url;
}

std::string request_url(const ChatRequest& request) {
  if (wire_of(request) == Wire::Google) {
    auto model = request.model;
    if (model.rfind("models/", 0) == 0) {
      model.erase(0, 7);
    }
    return google_base(request.api_url) + "/models/" + model +
           (request.stream ? ":streamGenerateContent?alt=sse" : ":generateContent");
  }
  if (wire_of(request) == Wire::Anthropic) {
    auto url = request.api_url;
    auto pos = url.find("/chat/completions");
    if (pos != std::string::npos) {
      url.replace(pos, 17, "/messages");
    }
    return url;
  }
  return request.api_url;
}

void merge_usage(Usage& into, const Usage& next) {
  if (next.input_tokens != 0) {
    into.input_tokens = next.input_tokens;
  }
  if (next.output_tokens != 0) {
    into.output_tokens = next.output_tokens;
  }
  into.cache_read_tokens = std::max(next.cache_read_tokens, into.cache_read_tokens);
  into.cache_write_tokens = std::max(next.cache_write_tokens, into.cache_write_tokens);
  into.cache_reported |= next.cache_reported;
}

void throw_if_error(const json& chunk) {
  if (!chunk.contains("error")) {
    return;
  }
  auto err = chunk["error"];
  throw Error(err.is_string() ? err.get<std::string>() : err.dump());
}

struct AnthropicStream {
  json content = json::array();
  std::vector<std::string> args;
};

void consume_anthropic(const ChatRequest& request, ChatResult& result, AnthropicStream& acc,
                       const json& chunk) {
  throw_if_error(chunk);
  if (!chunk.is_object()) {
    return;
  }
  auto type = chunk.value("type", "");
  if (type == "message_start" && chunk.contains("message") && chunk["message"].is_object()) {
    if (chunk["message"].contains("usage")) {
      merge_usage(result.usage, parse_chat_usage(chunk["message"]["usage"]));
    }
    return;
  }
  if (type == "content_block_start") {
    json block = chunk.value("content_block", json::object());
    acc.content.push_back(block.is_object() ? std::move(block) : json::object());
    acc.args.emplace_back();
    return;
  }
  if (type == "content_block_delta") {
    int i = chunk.value("index", 0);
    if (i < 0 || i >= static_cast<int>(acc.content.size())) {
      return;
    }
    json delta = chunk.value("delta", json::object());
    if (!delta.is_object()) {
      return;
    }
    auto dtype = delta.value("type", "");
    if (dtype == "text_delta") {
      auto piece = delta.value("text", "");
      if (!piece.empty()) {
        result.text += piece;
        emit(request, StreamEvent{EventKind::text_delta, piece, {}, {}});
      }
    } else if (dtype == "thinking_delta") {
      emit_thinking(request, delta.value("thinking", ""));
    } else if (dtype == "input_json_delta" && i < static_cast<int>(acc.args.size())) {
      acc.args[static_cast<size_t>(i)] += delta.value("partial_json", "");
    }
    return;
  }
  if (type == "content_block_stop") {
    int i = chunk.value("index", 0);
    if (i >= 0 && i < static_cast<int>(acc.args.size()) &&
        !acc.args[static_cast<size_t>(i)].empty()) {
      acc.content[static_cast<size_t>(i)]["input"] =
          detail::parse_tool_input(acc.args[static_cast<size_t>(i)]);
    }
    return;
  }
  if (type == "message_delta") {
    if (chunk.contains("usage")) {
      merge_usage(result.usage, parse_chat_usage(chunk["usage"]));
    }
    json delta = chunk.value("delta", json::object());
    auto reason = delta.is_object() ? delta.value("stop_reason", "") : "";
    if (reason == "tool_use") {
      result.finish_reason = "tool_calls";
    } else if (reason == "max_tokens") {
      result.finish_reason = "length";
    } else if (!reason.empty()) {
      result.finish_reason = "stop";
    }
  }
}

void finish_anthropic(ChatResult& result, AnthropicStream& acc) {
  for (const auto& block : acc.content) {
    if (block.value("type", "") != "tool_use") {
      continue;
    }
    ToolCall call;
    call.id = block.value("id", "");
    call.name = block.value("name", "");
    auto input = block.value("input", json::object());
    call.arguments = input.is_object() ? input.dump() : "{}";
    if (!call.id.empty()) {
      result.tool_calls.push_back(std::move(call));
    }
  }
}

void consume_google(const ChatRequest& request, ChatResult& result, std::map<int, ToolCall>& calls,
                    const json& chunk) {
  throw_if_error(chunk);
  if (!chunk.is_object()) {
    return;
  }
  if (chunk.contains("usageMetadata") && chunk["usageMetadata"].is_object()) {
    merge_usage(result.usage, parse_chat_usage(chunk["usageMetadata"]));
  }
  if (!chunk.contains("candidates") || !chunk["candidates"].is_array() ||
      chunk["candidates"].empty()) {
    return;
  }
  const auto& cand = chunk["candidates"][0];
  if (!cand.is_object()) {
    return;
  }
  auto reason = cand.contains("finishReason") && cand["finishReason"].is_string()
                    ? cand["finishReason"].get<std::string>()
                    : std::string();
  if (reason == "STOP") {
    result.finish_reason = "stop";
  } else if (reason == "MAX_TOKENS") {
    result.finish_reason = "length";
  }
  if (!cand.contains("content") || !cand["content"].is_object() ||
      !cand["content"].contains("parts") || !cand["content"]["parts"].is_array()) {
    return;
  }
  int i = 0;
  for (const auto& part : cand["content"]["parts"]) {
    if (!part.is_object()) {
      continue;
    }
    const bool thought =
        part.contains("thought") && part["thought"].is_boolean() && part["thought"].get<bool>();
    if (thought) {
      emit_thinking_value(request, part);
    } else if (part.contains("text") && part["text"].is_string()) {
      auto piece = part["text"].get<std::string>();
      if (!piece.empty()) {
        result.text += piece;
        emit(request, StreamEvent{EventKind::text_delta, piece, {}, {}});
      }
    }
    if (part.contains("functionCall") && part["functionCall"].is_object()) {
      const auto& fc = part["functionCall"];
      auto& call = calls[i];
      call.name = fc.value("name", "");
      if (call.id.empty()) {
        call.id = "google-call-" + std::to_string(i);
      }
      auto args = fc.contains("args") ? fc["args"] : json::object();
      call.arguments = args.is_object() ? args.dump() : "{}";
    }
    ++i;
  }
}

void consume_openai(const ChatRequest& request, ChatResult& result, std::map<int, ToolCall>& calls,
                    const json& chunk) {
  throw_if_error(chunk);
  if (!chunk.is_object()) {
    return;
  }
  if (chunk.contains("usage") && chunk["usage"].is_object()) {
    merge_usage(result.usage, parse_chat_usage(chunk["usage"]));
  }
  if (!chunk.contains("choices") || !chunk["choices"].is_array() || chunk["choices"].empty()) {
    return;
  }
  const auto& choice = chunk["choices"][0];
  if (!choice.is_object()) {
    return;
  }
  if (choice.contains("finish_reason") && choice["finish_reason"].is_string()) {
    result.finish_reason = choice["finish_reason"].get<std::string>();
  }
  if (!choice.contains("delta") || !choice["delta"].is_object()) {
    return;
  }
  const auto& delta = choice["delta"];
  for (const auto* key : {"reasoning_content", "reasoning", "reasoning_details"}) {
    if (delta.contains(key)) {
      emit_thinking_value(request, delta[key]);
    }
  }
  if (delta.contains("reasoning_content") && delta["reasoning_content"].is_string()) {
    result.reasoning_content += delta["reasoning_content"].get<std::string>();
  }
  if (delta.contains("reasoning_details") && delta["reasoning_details"].is_array()) {
    for (const auto& detail : delta["reasoning_details"]) {
      result.reasoning_details.push_back(detail);
    }
  }
  if (delta.contains("content")) {
    std::string piece;
    const auto& content = delta["content"];
    if (content.is_string()) {
      piece = content.get<std::string>();
    } else if (content.is_array()) {
      for (const auto& part : content) {
        if (part.is_string()) {
          piece += part.get<std::string>();
        } else if (part.is_object()) {
          const auto type = part.value("type", "");
          const bool thought = type == "reasoning" || type == "thinking" ||
                               (part.contains("thought") && part["thought"].is_boolean() &&
                                part["thought"].get<bool>());
          if (thought) {
            emit_thinking_value(request, part);
          } else {
            piece += part.value("text", "");
          }
        }
      }
    }
    if (!piece.empty()) {
      result.text += piece;
      emit(request, StreamEvent{EventKind::text_delta, piece, {}, {}});
    }
  }
  if (!delta.contains("tool_calls") || !delta["tool_calls"].is_array()) {
    return;
  }
  for (const auto& tc : delta["tool_calls"]) {
    if (!tc.is_object()) {
      continue;
    }
    int index = tc.value("index", 0);
    auto& acc = calls[index];
    if (tc.contains("id") && tc["id"].is_string()) {
      acc.id = tc["id"].get<std::string>();
    }
    if (tc.contains("function") && tc["function"].is_object()) {
      const auto& fn = tc["function"];
      if (fn.contains("name") && fn["name"].is_string()) {
        acc.name = fn["name"].get<std::string>();
      }
      if (fn.contains("arguments") && fn["arguments"].is_string()) {
        acc.arguments += fn["arguments"].get<std::string>();
      }
    }
  }
}

void collect_response_call(std::map<int, ToolCall>& calls, int index, const json& item) {
  if (!item.is_object() || item.value("type", "") != "function_call") {
    return;
  }
  auto& call = calls[index];
  call.id = item.value("call_id", "");
  call.name = item.value("name", "");
  if (item.contains("arguments") && item["arguments"].is_string()) {
    call.arguments = item["arguments"].get<std::string>();
  }
}

void consume_responses(const ChatRequest& request, ChatResult& result,
                       std::map<int, ToolCall>& calls, const json& event) {
  throw_if_error(event);
  if (!event.is_object()) {
    return;
  }
  const auto type = event.value("type", "");
  if (type == "error" || type == "response.failed") {
    const auto& response = event.value("response", json::object());
    const auto error = response.value("error", event.value("error", json::object()));
    const auto message = event.value("message", "");
    throw Error(error.is_object() ? error.value("message", message.empty() ? error.dump() : message)
                : error.is_string() ? error.get<std::string>()
                : message.empty()   ? event.dump()
                                    : message);
  }
  if (type == "response.output_text.delta" || type == "response.refusal.delta") {
    const auto piece = event.value("delta", "");
    if (!piece.empty()) {
      result.text += piece;
      emit(request, StreamEvent{EventKind::text_delta, piece, {}, {}});
    }
  } else if (type == "response.reasoning_summary_text.delta" ||
             type == "response.reasoning_text.delta") {
    const auto piece = event.value("delta", "");
    result.reasoning_content += piece;
    emit_thinking(request, piece);
  } else if (type == "response.function_call_arguments.delta" ||
             type == "response.function_call_arguments.done") {
    auto& call = calls[event.value("output_index", 0)];
    if (type == "response.function_call_arguments.delta") {
      call.arguments += event.value("delta", "");
    } else {
      call.arguments = event.value("arguments", call.arguments);
    }
  } else if (type == "response.output_item.added" || type == "response.output_item.done") {
    collect_response_call(calls, event.value("output_index", 0),
                          event.value("item", json::object()));
  } else if (type == "response.incomplete") {
    result.finish_reason = "length";
    const auto response = event.value("response", json::object());
    if (response.contains("usage")) {
      merge_usage(result.usage, parse_chat_usage(response["usage"]));
    }
  } else if (type == "response.completed") {
    const auto response = event.value("response", json::object());
    if (response.contains("usage")) {
      merge_usage(result.usage, parse_chat_usage(response["usage"]));
    }
    if (response.value("status", "") == "incomplete") {
      result.finish_reason = "length";
    }
    if (response.contains("output") && response["output"].is_array()) {
      for (size_t i = 0; i < response["output"].size(); ++i) {
        collect_response_call(calls, static_cast<int>(i), response["output"][i]);
      }
    }
  }
}

std::string responses_text(const json& body) {
  if (body.contains("output_text") && body["output_text"].is_string()) {
    return body["output_text"].get<std::string>();
  }
  if (!body.contains("output") || !body["output"].is_array()) {
    return {};
  }
  std::string text;
  for (const auto& item : body["output"]) {
    if (!item.is_object() || item.value("type", "") != "message" || !item.contains("content") ||
        !item["content"].is_array()) {
      continue;
    }
    for (const auto& part : item["content"]) {
      if (part.is_object() &&
          (part.value("type", "") == "output_text" || part.value("type", "") == "refusal")) {
        text += part.value("text", "");
      }
    }
  }
  return text;
}

std::string openai_text(const json& body) {
  if (!body.contains("choices") || !body["choices"].is_array() || body["choices"].empty() ||
      !body["choices"][0].is_object()) {
    return {};
  }
  const auto& message = body["choices"][0]["message"];
  if (!message.is_object() || !message.contains("content")) {
    return {};
  }
  const auto& content = message["content"];
  if (content.is_string()) {
    return content.get<std::string>();
  }
  return join_text(content);
}

std::string anthropic_text(const json& body) {
  if (!body.contains("content") || !body["content"].is_array()) {
    return {};
  }
  std::string text;
  for (const auto& part : body["content"]) {
    if (part.is_object() && part.value("type", "") == "text") {
      text += part.value("text", "");
    }
  }
  return text;
}

std::string google_text(const json& body) {
  if (!body.contains("candidates") || !body["candidates"].is_array() ||
      body["candidates"].empty() || !body["candidates"][0].is_object()) {
    return {};
  }
  const auto& cand = body["candidates"][0];
  if (!cand.contains("content") || !cand["content"].is_object() ||
      !cand["content"].contains("parts") || !cand["content"]["parts"].is_array()) {
    return {};
  }
  const auto& parts = cand["content"]["parts"];
  std::string text;
  for (const auto& part : parts) {
    if (part.is_object() && part.contains("text")) {
      text += part.value("text", "");
    }
  }
  return text;
}

} // namespace

json chat_body(const ChatRequest& request) {
  json payload;
  switch (wire_of(request)) {
  case Wire::Responses:
    payload = responses_body(request);
    break;
  case Wire::Anthropic:
    payload = anthropic_body(request);
    break;
  case Wire::Google:
    payload = google_body(request);
    break;
  case Wire::Chat:
    payload = openai_chat_body(request);
    break;
  }
  merge_extra(payload, request);
  return payload;
}

std::map<std::string, std::string> chat_headers(const ChatRequest& request) {
  std::map<std::string, std::string> headers{
      {"Content-Type", "application/json"},
  };
  const auto wire = wire_of(request);
  if (wire == Wire::Google) {
    headers["x-goog-api-key"] = request.api_key;
  } else if (!request.api_key.empty()) {
    headers["Authorization"] = "Bearer " + request.api_key;
  }
  if (wire == Wire::Anthropic) {
    headers["x-api-key"] = request.api_key;
  }
  if ((request.provider == "opencode" || request.provider == "opencodezen") &&
      !request.conversation_id.empty()) {
    headers["x-opencode-session"] = request.conversation_id;
  }
  for (const auto& [k, v] : request.extra_headers) {
    headers[k] = v;
  }
  return headers;
}

ChatResult stream_chat(ChatRequest&& req) {
  require_request(req);
  req.stream = true;
  json payload = chat_body(req);
  if (req.before_provider_request) {
    req.before_provider_request(payload);
  }
  HttpClient http;
  auto headers = chat_headers(req);
  headers["Accept"] = "text/event-stream";
  if (req.before_provider_headers) {
    req.before_provider_headers(headers);
  }

  ChatResult result;
  std::map<int, ToolCall> calls;
  AnthropicStream anthropic;
  auto wire = wire_of(req);

  if (auto streamed = http.post_sse(
          request_url(req), headers, payload.dump(),
          [&](std::string_view data) {
            if (req.cancel && req.cancel->load()) {
              return;
            }
            json chunk = json::parse(data);
            if (wire == Wire::Responses) {
              consume_responses(req, result, calls, chunk);
            } else if (wire == Wire::Anthropic) {
              consume_anthropic(req, result, anthropic, chunk);
            } else if (wire == Wire::Google) {
              consume_google(req, result, calls, chunk);
            } else {
              consume_openai(req, result, calls, chunk);
            }
          },
          req.cancel, req.after_provider_response);
      !streamed) {
    if (streamed.error().what() == std::string_view("interrupted")) {
      throw Cancelled();
    }
    throw streamed.error();
  }

  if (wire == Wire::Anthropic) {
    finish_anthropic(result, anthropic);
  }
  for (auto& [_, call] : calls) {
    if (call.id.empty() && call.name.empty()) {
      continue;
    }
    if (call.id.empty()) {
      call.id = call.name;
    }
    result.tool_calls.push_back(std::move(call));
  }
  if (result.finish_reason.empty()) {
    result.finish_reason = result.tool_calls.empty() ? "stop" : "tool_calls";
  }
  return result;
}

ChatResult stream_chat(const ChatRequest& request) {
  ChatRequest copy = request;
  return stream_chat(std::move(copy));
}

std::string complete_chat(const ChatRequest& request) {
  require_request(request);
  ChatRequest req = request;
  req.stream = false;
  json payload = chat_body(req);
  if (wire_of(req) == Wire::Chat && !payload.contains("max_tokens")) {
    payload["max_tokens"] = 4096;
  }
  if (req.before_provider_request) {
    req.before_provider_request(payload);
  }
  HttpClient http;
  auto headers = chat_headers(req);
  if (req.before_provider_headers) {
    req.before_provider_headers(headers);
  }
  auto res = http.post(request_url(req), headers, payload.dump());
  if (!res) {
    throw res.error();
  }
  if (req.after_provider_response) {
    req.after_provider_response(*res);
  }
  if (res->status >= 400) {
    throw Error("http " + std::to_string(res->status) + ": " + res->body);
  }
  json body = json::parse(res->body);
  throw_if_error(body);
  switch (wire_of(req)) {
  case Wire::Responses: {
    auto text = responses_text(body);
    if (text.empty() && !body.contains("output")) {
      throw Error("compaction: empty model response");
    }
    return text;
  }
  case Wire::Anthropic:
    return anthropic_text(body);
  case Wire::Google:
    return google_text(body);
  case Wire::Chat: {
    auto text = openai_text(body);
    if (text.empty() && (!body.contains("choices") || body["choices"].empty())) {
      throw Error("compaction: empty model response");
    }
    return text;
  }
  }
  std::unreachable();
}

Usage parse_chat_usage(const json& usage) {
  Usage out;
  if (!usage.is_object()) {
    return out;
  }
  auto count = [](const json& object, const char* key) {
    auto it = object.find(key);
    return it != object.end() && it->is_number_integer() ? it->get<int>() : 0;
  };
  out.input_tokens = count(usage, "prompt_tokens");
  if (out.input_tokens == 0) {
    out.input_tokens = count(usage, "input_tokens");
  }
  if (out.input_tokens == 0) {
    out.input_tokens = count(usage, "promptTokenCount");
  }
  out.output_tokens = count(usage, "completion_tokens");
  if (out.output_tokens == 0) {
    out.output_tokens = count(usage, "output_tokens");
  }
  if (out.output_tokens == 0) {
    out.output_tokens = count(usage, "candidatesTokenCount") + count(usage, "thoughtsTokenCount");
  }
  json details = json::object();
  if (usage.contains("prompt_tokens_details") && usage["prompt_tokens_details"].is_object()) {
    details = usage["prompt_tokens_details"];
  } else if (usage.contains("input_tokens_details") && usage["input_tokens_details"].is_object()) {
    details = usage["input_tokens_details"];
  }
  out.cache_read_tokens = count(details, "cached_tokens");
  out.cache_write_tokens = count(details, "cache_write_tokens");
  out.cache_reported = details.contains("cached_tokens") || details.contains("cache_write_tokens");
  if (usage.contains("cache_read_input_tokens")) {
    out.cache_read_tokens = count(usage, "cache_read_input_tokens");
    out.cache_reported = true;
  }
  if (usage.contains("cache_creation_input_tokens")) {
    out.cache_write_tokens = count(usage, "cache_creation_input_tokens");
    out.cache_reported = true;
  }
  if (usage.contains("cachedContentTokenCount")) {
    out.cache_read_tokens = count(usage, "cachedContentTokenCount");
    out.cache_reported = true;
  }
  if (out.cache_read_tokens > 0 || out.cache_write_tokens > 0) {
    out.cache_reported = true;
  }
  return out;
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
