#include "config.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>

namespace fs = std::filesystem;
using niminal::app::Config;
using niminal::app::load_config_file;
using niminal::app::save_config_file;

int main() {
  auto dir = fs::temp_directory_path() / "niminal-config-test";
  fs::remove_all(dir);
  fs::create_directories(dir);
  auto path = dir / "config.json";

  auto missing = load_config_file(path);
  if (missing.model != "openai/gpt-4o-mini" || missing.provider != "openrouter" ||
      missing.max_steps != 0 || missing.show_thinking || missing.editor != "" ||
      !missing.compaction_enabled ||
      missing.reserve_tokens != niminal::app::kDefaultReserveTokens ||
      missing.keep_recent_tokens != niminal::app::kDefaultKeepRecentTokens ||
      missing.context_window != 0) {
    std::cerr << "default model mismatch\n";
    return 1;
  }

  Config cfg;
  cfg.provider = "anthropic";
  cfg.model = "claude-sonnet-4-6";
  cfg.api_url = "https://api.anthropic.com/v1/chat/completions";
  cfg.last_models["openrouter"] = "openai/gpt-4o-mini";
  cfg.thinking = "high";
  cfg.show_thinking = true;
  cfg.editor = "hx";
  cfg.steering_mode = "all";
  cfg.follow_up_mode = "one-at-a-time";
  cfg.max_steps = 12;
  cfg.reserve_tokens = 32768;
  cfg.keep_recent_tokens = 40000;
  cfg.context_window = 256000;
  save_config_file(path, cfg);
  auto loaded = load_config_file(path);
  if (loaded.model != cfg.model || loaded.api_url != cfg.api_url ||
      loaded.provider != "anthropic" || loaded.thinking != "high" || !loaded.show_thinking ||
      loaded.editor != "hx" || loaded.steering_mode != "all" ||
      loaded.follow_up_mode != "one-at-a-time" || loaded.max_steps != 12 ||
      loaded.reserve_tokens != 32768 || loaded.keep_recent_tokens != 40000 ||
      loaded.context_window != 256000 || loaded.last_models["openrouter"] != "openai/gpt-4o-mini") {
    std::cerr << "roundtrip mismatch\n";
    return 1;
  }

  {
    std::ofstream out(path);
    out << R"({"compaction_enabled":false})";
  }
  auto disabled = load_config_file(path);
  if (disabled.compaction_enabled) {
    std::cerr << "compaction_enabled should load\n";
    return 1;
  }

  {
    std::ofstream out(path);
    out << R"({"reserve_tokens":-1,"keep_recent_tokens":-5,"context_window":-2})";
  }
  auto negative = load_config_file(path);
  if (negative.reserve_tokens != niminal::app::kDefaultReserveTokens ||
      negative.keep_recent_tokens != niminal::app::kDefaultKeepRecentTokens ||
      negative.context_window != 0) {
    std::cerr << "negative compaction values should use defaults\n";
    return 1;
  }

  {
    std::ofstream out(path);
    out << R"({"model":"x","api_url":"https://api.mistral.ai/v1/chat/completions"})";
  }
  auto inferred = load_config_file(path);
  if (inferred.provider != "mistral") {
    std::cerr << "infer provider from api_url\n";
    return 1;
  }

  {
    std::ofstream out(path);
    out << R"({"steering_mode":"bad","follow_up_mode":"all"})";
  }
  auto modes = load_config_file(path);
  if (modes.steering_mode != "one-at-a-time" || modes.follow_up_mode != "all") {
    std::cerr << "invalid queue mode should use defaults\n";
    return 1;
  }

  {
    std::ofstream out(path);
    out << R"({"max_steps":-1})";
  }
  auto unlimited = load_config_file(path);
  if (unlimited.max_steps != 0) {
    std::cerr << "negative max_steps should use unlimited default\n";
    return 1;
  }

  {
    std::ofstream out(path);
    out << "{not json";
  }
  auto bad = load_config_file(path);
  if (bad.model != "openai/gpt-4o-mini") {
    std::cerr << "invalid json should fall back to defaults\n";
    return 1;
  }

  fs::remove_all(dir);
  return 0;
}
