#include "models_dev.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>

namespace fs = std::filesystem;
using niminal::app::catalog_name;
using niminal::app::format_context_k;
using niminal::app::format_cost_usd;
using niminal::app::load_catalog;
using niminal::app::lookup_model_cost;
using niminal::app::lookup_reasoning_caps;
using niminal::app::search_catalog;
using niminal::app::set_catalog_cache_path;
using niminal::app::usage_cost_usd;

static int fail(const char* msg) {
  std::cerr << msg << '\n';
  return 1;
}

int main() {
  if (catalog_name("opencode") != "opencode-go" || catalog_name("opencodezen") != "opencode" ||
      catalog_name("OpenRouter") != "openrouter") {
    return fail("catalog_name");
  }
  if (format_context_k(200000) != "200k" || format_context_k(512) != "512") {
    return fail("format_context_k");
  }

  auto dir = fs::temp_directory_path() / "niminal-models-dev-test";
  fs::remove_all(dir);
  fs::create_directories(dir);
  auto path = dir / "models-dev.json";
  {
    std::ofstream out(path);
    out << R"({
      "anthropic": {"models": {
        "claude-sonnet-4-6": {"limit": {"context": 200000}, "reasoning": true,
          "cost": {"input": 3, "output": 15, "cache_read": 0.3, "cache_write": 3.75},
          "reasoning_options": [{"type": "effort", "values": ["low", "medium", "high"]}]},
        "claude-opus-4-6": {"limit": {"context": 200000}, "cost": {"input": 15}}
      }},
      "openrouter": {"models": {
        "anthropic/claude-sonnet-4": {"limit": {"context": 200000}},
        "deepseek/deepseek-v4-flash-0731": {"limit": {"context": 128000}},
        "z-ai/glm-5": {"limit": {"context": 200000}},
        "z-ai/glm-4.5": {"limit": {"context": 128000}}
      }}
    })";
  }
  set_catalog_cache_path(path);
  if (!load_catalog()) {
    return fail("load_catalog");
  }

  auto empty = search_catalog("anthropic", "", 50, {});
  if (empty.size() != 2) {
    return fail("empty query lists provider models");
  }

  auto clau = search_catalog("anthropic", "clau", 50, {});
  if (clau.size() != 2) {
    return fail("substring clau");
  }
  auto sonnet = search_catalog("anthropic", "sonnet", 50, {"claude-sonnet-4-6"});
  if (!sonnet.empty()) {
    return fail("skip recent");
  }
  auto orouter = search_catalog("openrouter", "deepseek", 50, {});
  if (orouter.size() != 1 || orouter[0].id != "deepseek/deepseek-v4-flash-0731") {
    return fail("openrouter id");
  }
  auto glm = search_catalog("openrouter", "glm", 50, {});
  if (glm.size() != 2 || glm[0].id != "z-ai/glm-4.5" || glm[1].id != "z-ai/glm-5") {
    return fail("provider model query");
  }
  if (!search_catalog("anthropic", "deepseek", 50, {}).empty()) {
    return fail("provider scoped");
  }
  auto caps = lookup_reasoning_caps("anthropic", "claude-sonnet-4-6");
  if (!caps.known || !caps.reasoning || caps.efforts.size() != 3) {
    return fail("reasoning caps");
  }
  if (lookup_reasoning_caps("anthropic", "missing").known) {
    return fail("unknown model");
  }

  auto cost = lookup_model_cost("anthropic", "claude-sonnet-4-6");
  if (!cost.known || cost.input != 3 || cost.output != 15 || cost.cache_read != 0.3 ||
      cost.cache_write != 3.75) {
    return fail("model cost");
  }
  if (lookup_model_cost("anthropic", "missing").known ||
      lookup_model_cost("openrouter", "claude-sonnet-4-6").known) {
    return fail("unknown cost");
  }
  // Anthropic style: input excludes cache reads and writes.
  auto anthropic_usage = niminal::Usage{500, 1'000, 10'000, 2'000, true};
  double anthropic_usd = usage_cost_usd(anthropic_usage, cost);
  double expected = (500 * 3 + 1'000 * 15 + 10'000 * 0.3 + 2'000 * 3.75) / 1'000'000.0;
  if (anthropic_usd != expected) {
    return fail("anthropic cost");
  }
  // OpenAI style: input already includes the cached reads.
  auto openai_usage = niminal::Usage{10'500, 1'000, 10'000, 0, true};
  double openai_usd = usage_cost_usd(openai_usage, cost);
  double expected_openai = (500 * 3 + 1'000 * 15 + 10'000 * 0.3) / 1'000'000.0;
  if (openai_usd != expected_openai) {
    return fail("openai cost");
  }
  if (format_cost_usd(0.25) != "$0.25" || format_cost_usd(1.234) != "$1.23" ||
      format_cost_usd(0.0042) != "$0.0042") {
    return fail("format cost");
  }

  fs::remove_all(dir);
  return 0;
}
