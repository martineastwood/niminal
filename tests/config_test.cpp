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
  if (missing.model != "openai/gpt-4o-mini" || missing.provider != "openrouter") {
    std::cerr << "default model mismatch\n";
    return 1;
  }

  Config cfg;
  cfg.provider = "anthropic";
  cfg.model = "claude-sonnet-4-6";
  cfg.api_url = "https://api.anthropic.com/v1/chat/completions";
  cfg.last_models["openrouter"] = "openai/gpt-4o-mini";
  cfg.thinking = "high";
  cfg.steering_mode = "all";
  cfg.follow_up_mode = "one-at-a-time";
  save_config_file(path, cfg);
  auto loaded = load_config_file(path);
  if (loaded.model != cfg.model || loaded.api_url != cfg.api_url ||
      loaded.provider != "anthropic" || loaded.thinking != "high" ||
      loaded.steering_mode != "all" ||
      loaded.follow_up_mode != "one-at-a-time" ||
      loaded.last_models["openrouter"] != "openai/gpt-4o-mini") {
    std::cerr << "roundtrip mismatch\n";
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
  if (modes.steering_mode != "one-at-a-time" ||
      modes.follow_up_mode != "all") {
    std::cerr << "invalid queue mode should use defaults\n";
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
