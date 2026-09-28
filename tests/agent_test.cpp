#include <cail/anthropic.hpp>
#include <cail/gemini.hpp>
#include <cail/openai.hpp>
#include <cail/openrouter.hpp>
#include <niminal/ai.hpp>

#include <algorithm>
#include <iostream>
#include <vector>

namespace {

niminal::json request_body(niminal::ChatRequest request) {
  niminal::json body;
  request.before_provider_request = [&](niminal::json& payload) {
    body = payload;
    throw niminal::Error("request captured");
  };
  try {
    niminal::stream_chat(request);
  } catch (const niminal::Error&) {
    if (body.is_null())
      throw;
  }
  return body;
}

} // namespace

void use_provider(niminal::ChatRequest& request, std::string provider, std::string model,
                  std::string url) {
  if (provider == "anthropic") {
    request.model = cail::create_anthropic(
        {.api_key = "test", .base_url = url, .max_tokens = 1024, .headers = {}})(model);
  } else if (provider == "google") {
    request.model = cail::create_gemini({.api_key = "test", .base_url = url, .headers = {}})(model);
  } else if (provider == "openai") {
    request.model = cail::create_openai({.api_key = "test", .base_url = url})(model);
  } else {
    request.model = cail::create_openrouter(
        {.api_key = "test", .headers = {}, .endpoint = url, .request_session_header = {}})(model);
  }
}

int main() {
  niminal::Agent agent;
  agent.system = "you are niminal";
  agent.system_extra = {"Project instructions.\n<file path=\"AGENTS.md\">\nbe brief\n</file>\n"};
  agent.messages = niminal::json_array({niminal::json{{"role", "user"}, {"content", "hello"}}});
  auto msgs = agent.request_messages();
  if (!msgs.is_array() || msgs.size() != 2) {
    std::cerr << "expected system + user\n";
    return 1;
  }
  if (niminal::json_value(msgs[0], "role", "") != "system" || !msgs[0]["content"].is_array() ||
      msgs[0]["content"].size() != 2) {
    std::cerr << "system should be two stable text parts\n";
    return 1;
  }
  if (niminal::json_value(msgs[0]["content"][0], "text", "") != "you are niminal") {
    std::cerr << "base system first\n";
    return 1;
  }
  if (niminal::json_value(msgs[0]["content"][1], "text", "").find("AGENTS.md") ==
      std::string::npos) {
    std::cerr << "instructions after base system\n";
    return 1;
  }
  if (niminal::json_value(msgs[1], "role", "") != "user") {
    std::cerr << "conversation after system prefix\n";
    return 1;
  }

  niminal::ChatRequest req;
  const auto endpoint = "http://127.0.0.1:1";
  use_provider(req, "openrouter", "openai/gpt-4o-mini",
               std::string(endpoint) + "/v1/chat/completions");
  req.messages = msgs;
  req.tools = niminal::json_array({niminal::json{
      {"type", "function"},
      {"function",
       {{"name", "read"},
        {"description", "d"},
        {"parameters", {{"type", "object"}, {"properties", niminal::json_object()}}}}},
  }});
  auto body = request_body(req);
  if (body["tools"][0].contains("cache_control")) {
    std::cerr << "openai models should not send cache_control\n";
    return 1;
  }
  if (body["messages"][0]["content"].get_array().back().contains("cache_control")) {
    std::cerr << "openai system should not send cache_control\n";
    return 1;
  }
  if (body["messages"].get_array().back()["content"].get<std::string>() != "hello") {
    std::cerr << "user text should reach the provider\n";
    return 1;
  }
  use_provider(req, "openrouter", "anthropic/claude-sonnet-4",
               std::string(endpoint) + "/v1/chat/completions");
  req.apply_cache = true;
  auto claude = request_body(req);
  if (!claude["tools"][0].contains("cache_control")) {
    std::cerr << "last tool should carry cache_control\n";
    return 1;
  }
  if (!claude["messages"][0]["content"].get_array().back().contains("cache_control")) {
    std::cerr << "last system part should carry cache_control\n";
    return 1;
  }
  if (!claude["messages"].get_array().back()["content"].is_array() ||
      !claude["messages"].get_array().back()["content"].get_array().back().contains(
          "cache_control")) {
    std::cerr << "last message should carry cache_control\n";
    return 1;
  }
  if (!body.contains("stream_options") ||
      !niminal::json_value(body["stream_options"], "include_usage", false)) {
    std::cerr << "stream should request usage\n";
    return 1;
  }
  req.apply_cache = false;
  use_provider(req, "google", "gemini-3.5-flash-lite",
               "https://generativelanguage.googleapis.com/v1beta");
  req.stream_usage = false;
  req.conversation_id = "sess";
  auto gemini = request_body(req);
  if (!gemini.contains("systemInstruction") || !gemini.contains("contents")) {
    std::cerr << "google should use generateContent\n";
    return 1;
  }
  if (gemini.contains("messages") || gemini.contains("cache_control") ||
      niminal::json_dump(gemini).find("cache_control") != std::string::npos) {
    std::cerr << "google should not send cache_control\n";
    return 1;
  }
  req.apply_cache = true;
  use_provider(req, "anthropic", "claude-sonnet-4-6", "https://api.anthropic.com/v1");
  auto native = request_body(req);
  if (!native.contains("system") || !native["system"].is_array() ||
      !native["system"].get_array().back().contains("cache_control")) {
    std::cerr << "anthropic system should be cached\n";
    return 1;
  }
  if (!native.contains("tools") || !native["tools"][0].contains("input_schema") ||
      !native["tools"][0].contains("cache_control")) {
    std::cerr << "anthropic tools should use Messages cache_control\n";
    return 1;
  }
  if (!native["messages"].get_array().back()["content"].get_array().back().contains(
          "cache_control")) {
    std::cerr << "anthropic last message should be cached\n";
    return 1;
  }
  if (native.contains("choices") || native.contains("stream_options")) {
    std::cerr << "anthropic should not use Chat Completions fields\n";
    return 1;
  }
  use_provider(req, "openai", "gpt-5", "https://api.openai.com/v1");
  req.apply_cache = false;
  req.conversation_id = "sess";
  auto oai = request_body(req);
  if (niminal::json_dump(oai).find("cache_control") != std::string::npos) {
    std::cerr << "openai should cache by prefix, not cache_control\n";
    return 1;
  }
  if (niminal::json_value(oai, "prompt_cache_key", "") != "sess") {
    std::cerr << "openai should send prompt_cache_key\n";
    return 1;
  }
  req.extra = niminal::json{{"reasoning", {{"effort", "high"}}}};
  auto reasoned = request_body(req);
  if (niminal::json_value(reasoned["reasoning"], "effort", "") != "high") {
    std::cerr << "chat extra reasoning\n";
    return 1;
  }
  req.apply_cache = false;
  use_provider(req, "google", "gemini-3.5-flash",
               "https://generativelanguage.googleapis.com/v1beta");
  req.extra = niminal::json{{"reasoning_effort", "high"}};
  auto gthink = request_body(req);
  if (niminal::json_value(gthink["generationConfig"]["thinkingConfig"], "thinkingLevel", "") !=
      "HIGH") {
    std::cerr << "google thinkingLevel\n";
    return 1;
  }

  const niminal::json image = {
      {"type", "image"}, {"name", "screen.png"}, {"mime_type", "image/png"}, {"data", "aGVsbG8="}};
  req.messages = niminal::json_array(
      {{{"role", "user"},
        {"content", niminal::json_array({{{"type", "text"}, {"text", "inspect"}}, image})}},
       {{"role", "assistant"},
        {"content", ""},
        {"tool_calls",
         niminal::json_array({{{"id", "t1"},
                               {"type", "function"},
                               {"function", {{"name", "read"}, {"arguments", "{}"}}}}})}},
       {{"role", "tool"},
        {"tool_call_id", "t1"},
        {"content", "screen.png"},
        {"images", niminal::json_array({image})}}});
  req.extra = niminal::json_object();
  use_provider(req, "openai", "gpt-5", "https://api.openai.com/v1");
  auto vision = request_body(req);
  if (vision["input"][0]["content"][1]["image_url"].get<std::string>() !=
          "data:image/png;base64,aGVsbG8=" ||
      vision["input"].get_array().back()["content"][0]["type"].get<std::string>() !=
          "input_image") {
    std::cerr << "OpenAI image content\n";
    return 1;
  }
  req.apply_cache = true;
  use_provider(req, "anthropic", "claude-sonnet-4-6", "https://api.anthropic.com/v1");
  vision = request_body(req);
  if (vision["messages"][0]["content"][1]["source"]["data"].get<std::string>() != "aGVsbG8=" ||
      vision["messages"]
              .get_array()
              .back()["content"][0]["content"][1]["source"]["data"]
              .get<std::string>() != "aGVsbG8=") {
    std::cerr << "Anthropic image content\n";
    return 1;
  }
  req.apply_cache = false;
  use_provider(req, "google", "gemini-3.5-flash",
               "https://generativelanguage.googleapis.com/v1beta");
  vision = request_body(req);
  if (vision["contents"][0]["parts"][1]["inlineData"]["data"].get<std::string>() != "aGVsbG8=" ||
      vision["contents"].get_array().back()["parts"][1]["inlineData"]["data"].get<std::string>() !=
          "aGVsbG8=") {
    std::cerr << "Google image content\n";
    return 1;
  }

  const niminal::Usage usage{100, 20, 80, 0, true};
  auto line = niminal::format_usage_line(usage);
  if (line.find("↑100") == std::string::npos || line.find("↓20") == std::string::npos ||
      line.find("CH80.0%") == std::string::npos) {
    std::cerr << "format_usage_line: " << line << '\n';
    return 1;
  }
  auto wrote = niminal::format_usage_line(niminal::Usage{100, 5, 0, 80, true});
  if (wrote.find("W80") == std::string::npos || wrote.find("CH") != std::string::npos) {
    std::cerr << "format write: " << wrote << '\n';
    return 1;
  }

  if (!niminal::format_context_percent(0, 128'000).empty()) {
    std::cerr << "format_context_percent empty usage\n";
    return 1;
  }
  if (!niminal::format_context_percent(64'000, 0).empty()) {
    std::cerr << "format_context_percent unknown window\n";
    return 1;
  }
  if (niminal::format_context_percent(64'000, 128'000) != "context 50%") {
    std::cerr << "format_context_percent half: " << niminal::format_context_percent(64'000, 128'000)
              << '\n';
    return 1;
  }
  if (niminal::format_context_percent(200'000, 128'000) != "context 100%") {
    std::cerr << "format_context_percent clamped: "
              << niminal::format_context_percent(200'000, 128'000) << '\n';
    return 1;
  }

  niminal::Cancellation cancel;
  cancel.request();
  niminal::Agent cancelled;
  cancelled.model = "test-model";
  cancelled.cancel = &cancel;
  std::vector<niminal::EventKind> kinds;
  bool turn_interrupted = false;
  cancelled.turn_end = [&](bool interrupted) { turn_interrupted = interrupted; };
  cancelled.on_event = [&](const niminal::StreamEvent& event) {
    if (event.kind != niminal::EventKind::text_delta) {
      kinds.push_back(event.kind);
    }
  };
  cancelled.run("hello");
  const bool saw_interrupt =
      std::find(kinds.begin(), kinds.end(), niminal::EventKind::interrupted) != kinds.end();
  const bool saw_error =
      std::find(kinds.begin(), kinds.end(), niminal::EventKind::error) != kinds.end();
  if (!saw_interrupt || saw_error) {
    std::cerr << "cancelling a run should emit an interrupted event and no error event\n";
    return 1;
  }
  if (!turn_interrupted) {
    std::cerr << "cancelling a run should report an interrupted turn\n";
    return 1;
  }

  return 0;
}
