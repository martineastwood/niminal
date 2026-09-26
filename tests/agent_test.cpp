#include <niminal/ai.hpp>

#include <iostream>

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
    if (body.is_null()) throw;
  }
  return body;
}

} // namespace

int main() {
  niminal::Agent agent;
  agent.system = "you are niminal";
  agent.system_extra = {"Project instructions.\n<file path=\"AGENTS.md\">\nbe brief\n</file>\n"};
  agent.messages = nlohmann::json::array({nlohmann::json{{"role", "user"}, {"content", "hello"}}});
  auto msgs = agent.request_messages();
  if (!msgs.is_array() || msgs.size() != 2) {
    std::cerr << "expected system + user\n";
    return 1;
  }
  if (msgs[0].value("role", "") != "system" || !msgs[0]["content"].is_array() ||
      msgs[0]["content"].size() != 2) {
    std::cerr << "system should be two stable text parts\n";
    return 1;
  }
  if (msgs[0]["content"][0].value("text", "") != "you are niminal") {
    std::cerr << "base system first\n";
    return 1;
  }
  if (msgs[0]["content"][1].value("text", "").find("AGENTS.md") == std::string::npos) {
    std::cerr << "instructions after base system\n";
    return 1;
  }
  if (msgs[1].value("role", "") != "user") {
    std::cerr << "conversation after system prefix\n";
    return 1;
  }

  niminal::ChatRequest req;
  req.provider = "openrouter";
  req.api_key = "test";
  req.api_url = "http://127.0.0.1:1/v1/chat/completions";
  req.model = "openai/gpt-4o-mini";
  req.messages = msgs;
  req.tools = nlohmann::json::array({nlohmann::json{
      {"type", "function"},
      {"function",
       {{"name", "read"}, {"description", "d"}, {"parameters", {{"type", "object"}, {"properties", nlohmann::json::object()}}}}},
  }});
  auto body = request_body(req);
  if (body["tools"][0].contains("cache_control")) {
    std::cerr << "openai models should not send cache_control\n";
    return 1;
  }
  if (body["messages"][0]["content"].back().contains("cache_control")) {
    std::cerr << "openai system should not send cache_control\n";
    return 1;
  }
  if (body["messages"].back()["content"] != "hello") {
    std::cerr << "user text should reach the provider\n";
    return 1;
  }
  req.model = "anthropic/claude-sonnet-4";
  req.apply_cache = true;
  auto claude = request_body(req);
  if (!claude["tools"][0].contains("cache_control")) {
    std::cerr << "last tool should carry cache_control\n";
    return 1;
  }
  if (!claude["messages"][0]["content"].back().contains("cache_control")) {
    std::cerr << "last system part should carry cache_control\n";
    return 1;
  }
  if (!claude["messages"].back()["content"].is_array() ||
      !claude["messages"].back()["content"].back().contains("cache_control")) {
    std::cerr << "last message should carry cache_control\n";
    return 1;
  }
  if (!body.contains("stream_options") || !body["stream_options"].value("include_usage", false)) {
    std::cerr << "stream should request usage\n";
    return 1;
  }
  req.apply_cache = false;
  req.provider = "google";
  req.model = "gemini-3.5-flash-lite";
  req.stream_usage = false;
  req.conversation_id = "sess";
  auto gemini = request_body(req);
  if (!gemini.contains("systemInstruction") || !gemini.contains("contents")) {
    std::cerr << "google should use generateContent\n";
    return 1;
  }
  if (gemini.contains("messages") || gemini.contains("cache_control") ||
      gemini.dump().find("cache_control") != std::string::npos) {
    std::cerr << "google should not send cache_control\n";
    return 1;
  }
  req.apply_cache = true;
  req.provider = "anthropic";
  req.model = "claude-sonnet-4-6";
  auto native = request_body(req);
  if (!native.contains("system") || !native["system"].is_array() ||
      !native["system"].back().contains("cache_control")) {
    std::cerr << "anthropic system should be cached\n";
    return 1;
  }
  if (!native.contains("tools") || !native["tools"][0].contains("input_schema") ||
      !native["tools"][0].contains("cache_control")) {
    std::cerr << "anthropic tools should use Messages cache_control\n";
    return 1;
  }
  if (!native["messages"].back()["content"].back().contains("cache_control")) {
    std::cerr << "anthropic last message should be cached\n";
    return 1;
  }
  if (native.contains("choices") || native.contains("stream_options")) {
    std::cerr << "anthropic should not use Chat Completions fields\n";
    return 1;
  }
  req.provider = "openai";
  req.model = "gpt-5";
  req.apply_cache = false;
  req.prompt_cache_key = true;
  req.conversation_id = "sess";
  auto oai = request_body(req);
  if (oai.dump().find("cache_control") != std::string::npos) {
    std::cerr << "openai should cache by prefix, not cache_control\n";
    return 1;
  }
  if (oai.value("prompt_cache_key", "") != "sess") {
    std::cerr << "openai should send prompt_cache_key\n";
    return 1;
  }
  req.extra = nlohmann::json{{"reasoning", {{"effort", "high"}}}};
  auto reasoned = request_body(req);
  if (reasoned["reasoning"].value("effort", "") != "high") {
    std::cerr << "chat extra reasoning\n";
    return 1;
  }
  req.apply_cache = false;
  req.provider = "google";
  req.model = "gemini-3.5-flash";
  req.extra = nlohmann::json{{"reasoning_effort", "high"}};
  auto gthink = request_body(req);
  if (gthink["generationConfig"]["thinkingConfig"].value("thinkingLevel", "") != "HIGH") {
    std::cerr << "google thinkingLevel\n";
    return 1;
  }

  const nlohmann::json image = {
      {"type", "image"}, {"name", "screen.png"}, {"mime_type", "image/png"}, {"data", "aGVsbG8="}};
  req.messages = nlohmann::json::array(
      {{{"role", "user"},
        {"content", nlohmann::json::array({{{"type", "text"}, {"text", "inspect"}}, image})}},
       {{"role", "assistant"},
        {"content", ""},
        {"tool_calls",
         nlohmann::json::array({{{"id", "t1"},
                                 {"type", "function"},
                                 {"function", {{"name", "read"}, {"arguments", "{}"}}}}})}},
       {{"role", "tool"},
        {"tool_call_id", "t1"},
        {"content", "screen.png"},
        {"images", nlohmann::json::array({image})}}});
  req.extra = nlohmann::json::object();
  req.provider = "openai";
  auto vision = request_body(req);
  if (vision["input"][0]["content"][1]["image_url"] != "data:image/png;base64,aGVsbG8=" ||
      vision["input"].back()["content"][0]["type"] != "input_image") {
    std::cerr << "OpenAI image content\n";
    return 1;
  }
  req.apply_cache = true;
  req.provider = "anthropic";
  vision = request_body(req);
  if (vision["messages"][0]["content"][1]["source"]["data"] != "aGVsbG8=" ||
      vision["messages"].back()["content"][0]["content"][1]["source"]["data"] != "aGVsbG8=") {
    std::cerr << "Anthropic image content\n";
    return 1;
  }
  req.apply_cache = false;
  req.provider = "google";
  vision = request_body(req);
  if (vision["contents"][0]["parts"][1]["inlineData"]["data"] != "aGVsbG8=" ||
      vision["contents"].back()["parts"][1]["inlineData"]["data"] != "aGVsbG8=") {
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
  return 0;
}
