#include <niminal/ai.hpp>

#include <iostream>

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
  req.model = "openai/gpt-4o-mini";
  req.messages = msgs;
  req.tools = nlohmann::json::array({nlohmann::json{
      {"type", "function"},
      {"function",
       {{"name", "read"}, {"description", "d"}, {"parameters", nlohmann::json::object()}}},
  }});
  auto body = niminal::chat_body(req);
  if (body["tools"][0].contains("cache_control")) {
    std::cerr << "openai models should not send cache_control\n";
    return 1;
  }
  if (body["messages"][0]["content"].back().contains("cache_control")) {
    std::cerr << "openai system should not send cache_control\n";
    return 1;
  }
  if (!body["messages"].back()["content"].is_array()) {
    std::cerr << "user content should be a text array for a stable prefix\n";
    return 1;
  }
  req.model = "anthropic/claude-sonnet-4";
  auto claude = niminal::chat_body(req);
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
  req.provider = "google";
  req.model = "gemini-3.5-flash-lite";
  req.stream_usage = false;
  req.conversation_id = "sess";
  auto gemini = niminal::chat_body(req);
  if (!gemini.contains("systemInstruction") || !gemini.contains("contents")) {
    std::cerr << "google should use generateContent\n";
    return 1;
  }
  if (gemini.contains("messages") || gemini.contains("cache_control") ||
      gemini.dump().find("cache_control") != std::string::npos) {
    std::cerr << "google should not send cache_control\n";
    return 1;
  }
  req.provider = "anthropic";
  req.model = "claude-sonnet-4-6";
  auto native = niminal::chat_body(req);
  if (!native.contains("system") || !native["system"].is_array() ||
      !native["system"].back().contains("cache_control")) {
    std::cerr << "anthropic system should be cached\n";
    return 1;
  }
  if (!native.contains("tools") || native["tools"][0].value("type", "") != "custom" ||
      !native["tools"][0].contains("input_schema") ||
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
  auto oai = niminal::chat_body(req);
  if (oai.dump().find("cache_control") != std::string::npos) {
    std::cerr << "openai should cache by prefix, not cache_control\n";
    return 1;
  }
  if (oai.value("prompt_cache_key", "") != "sess") {
    std::cerr << "openai should send prompt_cache_key\n";
    return 1;
  }
  req.extra = nlohmann::json{{"reasoning", {{"effort", "high"}}}};
  auto reasoned = niminal::chat_body(req);
  if (reasoned["reasoning"].value("effort", "") != "high") {
    std::cerr << "chat extra reasoning\n";
    return 1;
  }
  req.provider = "google";
  req.model = "gemini-3.5-flash";
  req.extra = nlohmann::json{{"reasoning_effort", "high"}};
  auto gthink = niminal::chat_body(req);
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
  auto vision = niminal::chat_body(req);
  if (vision["messages"][0]["content"][1]["image_url"]["url"] != "data:image/png;base64,aGVsbG8=" ||
      vision["messages"].back()["content"][0]["type"] != "image_url") {
    std::cerr << "OpenAI image content\n";
    return 1;
  }
  req.provider = "anthropic";
  vision = niminal::chat_body(req);
  if (vision["messages"][0]["content"][1]["source"]["data"] != "aGVsbG8=" ||
      vision["messages"].back()["content"][0]["content"][1]["source"]["data"] != "aGVsbG8=") {
    std::cerr << "Anthropic image content\n";
    return 1;
  }
  req.provider = "google";
  vision = niminal::chat_body(req);
  if (vision["contents"][0]["parts"][1]["inlineData"]["data"] != "aGVsbG8=" ||
      vision["contents"].back()["parts"][1]["inlineData"]["data"] != "aGVsbG8=") {
    std::cerr << "Google image content\n";
    return 1;
  }

  auto usage = niminal::parse_chat_usage(nlohmann::json{
      {"prompt_tokens", 100},
      {"completion_tokens", 20},
      {"prompt_tokens_details", {{"cached_tokens", 80}}},
  });
  if (usage.input_tokens != 100 || usage.output_tokens != 20 || usage.cache_read_tokens != 80 ||
      !usage.cache_reported) {
    std::cerr << "parse_chat_usage\n";
    return 1;
  }
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
  auto google_usage = niminal::parse_chat_usage(nlohmann::json{
      {"promptTokenCount", 50},
      {"candidatesTokenCount", 10},
      {"cachedContentTokenCount", 40},
  });
  if (google_usage.input_tokens != 50 || google_usage.output_tokens != 10 ||
      google_usage.cache_read_tokens != 40 || !google_usage.cache_reported) {
    std::cerr << "google usage\n";
    return 1;
  }
  auto null_usage = niminal::parse_chat_usage(nlohmann::json{
      {"prompt_tokens", nullptr},
      {"completion_tokens", nullptr},
      {"cache_read_input_tokens", nullptr},
  });
  if ((null_usage.input_tokens != 0) || (null_usage.output_tokens != 0) ||
      (null_usage.cache_read_tokens != 0)) {
    std::cerr << "null usage\n";
    return 1;
  }
  return 0;
}
