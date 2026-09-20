#include "config.hpp"

#include <nlohmann/json.hpp>

#include <cstdlib>
#include <fstream>
#include <sstream>
#include <stdexcept>
#include <system_error>

namespace niminal::app {
namespace fs = std::filesystem;
using json = nlohmann::json;

namespace {

bool valid_queue_mode(const std::string& mode) {
  return mode == "all" || mode == "one-at-a-time";
}

void load_queue_mode(const json& doc, const char* key, std::string& target) {
  if (doc.contains(key) && doc[key].is_string() &&
      valid_queue_mode(doc[key].get<std::string>()))
    target = doc[key].get<std::string>();
}

}  // namespace

std::filesystem::path config_path() {
  const char* home = std::getenv("HOME");
  if (!home || !*home)
    throw std::runtime_error("HOME is not set; cannot load ~/.niminal/config.json");
  return fs::path(home) / ".niminal" / "config.json";
}

Config load_config_file(const fs::path& path) {
  Config cfg;
  std::error_code ec;
  if (!fs::exists(path, ec)) return cfg;
  std::ifstream in(path);
  if (!in) return cfg;
  std::ostringstream ss;
  ss << in.rdbuf();
  try {
    auto doc = json::parse(ss.str());
    if (doc.contains("model") && doc["model"].is_string()) {
      auto model = doc["model"].get<std::string>();
      if (!model.empty()) cfg.model = std::move(model);
    }
    if (doc.contains("api_url") && doc["api_url"].is_string()) {
      auto url = doc["api_url"].get<std::string>();
      if (!url.empty()) cfg.api_url = std::move(url);
    }
    if (doc.contains("provider") && doc["provider"].is_string()) {
      auto provider = doc["provider"].get<std::string>();
      if (!provider.empty()) cfg.provider = std::move(provider);
    } else if (!cfg.api_url.empty()) {
      const auto& url = cfg.api_url;
      if (url.find("api.anthropic.com") != std::string::npos)
        cfg.provider = "anthropic";
      else if (url.find("generativelanguage.googleapis.com") != std::string::npos)
        cfg.provider = "google";
      else if (url.find("hyper.charm.land") != std::string::npos)
        cfg.provider = "hyper";
      else if (url.find("api.mistral.ai") != std::string::npos)
        cfg.provider = "mistral";
      else if (url.find("api.openai.com") != std::string::npos)
        cfg.provider = "openai";
      else if (url.find("opencode.ai/zen/go") != std::string::npos)
        cfg.provider = "opencode";
      else if (url.find("opencode.ai") != std::string::npos)
        cfg.provider = "opencodezen";
    }
    if (doc.contains("thinking") && doc["thinking"].is_string())
      cfg.thinking = doc["thinking"].get<std::string>();
    if (doc.contains("show_thinking") && doc["show_thinking"].is_boolean())
      cfg.show_thinking = doc["show_thinking"].get<bool>();
    if (doc.contains("theme") && doc["theme"].is_string())
      cfg.theme = doc["theme"].get<std::string>();
    load_queue_mode(doc, "steering_mode", cfg.steering_mode);
    load_queue_mode(doc, "follow_up_mode", cfg.follow_up_mode);
    if (doc.contains("max_steps") && doc["max_steps"].is_number_integer()) {
      const int max_steps = doc["max_steps"].get<int>();
      if (max_steps >= 0) cfg.max_steps = max_steps;
    }
    if (doc.contains("compaction_enabled") && doc["compaction_enabled"].is_boolean())
      cfg.compaction_enabled = doc["compaction_enabled"].get<bool>();
    if (doc.contains("reserve_tokens") && doc["reserve_tokens"].is_number_integer() &&
        doc["reserve_tokens"].get<int>() >= 0)
      cfg.reserve_tokens = doc["reserve_tokens"].get<int>();
    if (doc.contains("keep_recent_tokens") &&
        doc["keep_recent_tokens"].is_number_integer() &&
        doc["keep_recent_tokens"].get<int>() >= 0)
      cfg.keep_recent_tokens = doc["keep_recent_tokens"].get<int>();
    if (doc.contains("context_window") && doc["context_window"].is_number_integer() &&
        doc["context_window"].get<int>() >= 0)
      cfg.context_window = doc["context_window"].get<int>();
    if (doc.contains("providers") && doc["providers"].is_object()) {
      for (auto& [name, block] : doc["providers"].items()) {
        if (block.is_object() && block.contains("last_model") &&
            block["last_model"].is_string()) {
          auto model = block["last_model"].get<std::string>();
          if (!model.empty()) cfg.last_models[name] = std::move(model);
        }
      }
    }
  } catch (...) {
    return Config{};
  }
  return cfg;
}

Config load_config() {
  try {
    return load_config_file(config_path());
  } catch (...) {
    return Config{};
  }
}

void save_config_file(const fs::path& path, const Config& cfg) {
  fs::create_directories(path.parent_path());
  json doc = {{"provider", cfg.provider},
              {"model", cfg.model},
              {"show_thinking", cfg.show_thinking},
              {"theme", cfg.theme},
              {"steering_mode", cfg.steering_mode},
              {"follow_up_mode", cfg.follow_up_mode}};
  if (cfg.max_steps > 0) doc["max_steps"] = cfg.max_steps;
  if (!cfg.compaction_enabled) doc["compaction_enabled"] = false;
  if (cfg.reserve_tokens != kDefaultReserveTokens)
    doc["reserve_tokens"] = cfg.reserve_tokens;
  if (cfg.keep_recent_tokens != kDefaultKeepRecentTokens)
    doc["keep_recent_tokens"] = cfg.keep_recent_tokens;
  if (cfg.context_window > 0) doc["context_window"] = cfg.context_window;
  if (!cfg.api_url.empty()) doc["api_url"] = cfg.api_url;
  if (!cfg.thinking.empty()) doc["thinking"] = cfg.thinking;
  if (!cfg.last_models.empty()) {
    json providers = json::object();
    for (const auto& [name, model] : cfg.last_models)
      providers[name] = json{{"last_model", model}};
    doc["providers"] = std::move(providers);
  }
  auto tmp = path;
  tmp += ".tmp";
  {
    std::ofstream out(tmp, std::ios::binary | std::ios::trunc);
    if (!out) throw std::runtime_error("cannot write " + path.string());
    out << doc.dump(2) << '\n';
  }
  fs::rename(tmp, path);
}

void save_config(const Config& cfg) { save_config_file(config_path(), cfg); }

}  // namespace niminal::app
