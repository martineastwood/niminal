#include "provider.hpp"

#include <iostream>
#include <string>

using niminal::app::Config;
using niminal::app::apply_provider;
using niminal::app::find_provider;
using niminal::app::infer_provider;
using niminal::app::provider_names;
using niminal::app::select_provider;

static int fail(const char* msg) {
  std::cerr << msg << '\n';
  return 1;
}

int main() {
  if (!find_provider("anthropic") || !find_provider("GOOGLE") ||
      find_provider("codex") || find_provider("gemini"))
    return fail("find_provider");
  auto names = provider_names();
  if (names.find("openrouter") == std::string::npos ||
      names.find("mistral") == std::string::npos)
    return fail("provider_names");
  if (infer_provider("https://api.anthropic.com/v1/messages") != "anthropic" ||
      infer_provider("https://generativelanguage.googleapis.com/v1beta") !=
          "google" ||
      infer_provider("https://opencode.ai/zen/go/v1/chat/completions") !=
          "opencode")
    return fail("infer_provider");

  Config cfg;
  std::string err;
  if (!select_provider(cfg, "anthropic", &err)) return fail(err.c_str());
  if (cfg.provider != "anthropic" || cfg.model != "claude-sonnet-4-6")
    return fail("select anthropic default model");
  if (cfg.last_models["openrouter"] != "openai/gpt-4o-mini")
    return fail("remember previous model");
  cfg.model = "claude-opus-4-6";
  if (!select_provider(cfg, "openai", &err)) return fail(err.c_str());
  if (cfg.model != "gpt-5") return fail("openai default");
  if (!select_provider(cfg, "anthropic", &err)) return fail(err.c_str());
  if (cfg.model != "claude-opus-4-6") return fail("restore last model");
  if (select_provider(cfg, "codex", &err)) return fail("codex should fail");
  if (err.find("codex") == std::string::npos) return fail("codex error");
  if (select_provider(cfg, "nope", &err)) return fail("unknown should fail");

  niminal::Agent agent;
  apply_provider(agent, cfg);
  if (agent.provider != "anthropic" ||
      agent.api_url.find("/v1/messages") == std::string::npos ||
      agent.extra_headers["anthropic-version"] != "2023-06-01" ||
      !agent.apply_cache || agent.session_routing || agent.stream_usage ||
      agent.key_hint != "ANTHROPIC_API_KEY")
    return fail("apply_provider anthropic");

  Config stale;
  stale.provider = "anthropic";
  stale.model = "claude-haiku-4-5";
  stale.api_url = "https://api.anthropic.com/v1/chat/completions";
  apply_provider(agent, stale);
  if (agent.api_url.find("/v1/messages") == std::string::npos)
    return fail("stale chat/completions url should map to messages");
  return 0;
}
