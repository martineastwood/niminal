#include "config.hpp"
#include "queue_mode.hpp"

#include <niminal/providers.hpp>

#include <nlohmann/json.hpp>

#include <algorithm>
#include <cstdlib>
#include <fstream>
#include <stdexcept>
#include <system_error>

namespace niminal::app {
namespace fs = std::filesystem;
using json = nlohmann::json;

namespace {

void load_queue_mode(const json& doc, const char* key, std::string& target) {
  if (doc.contains(key) && doc[key].is_string() && valid_queue_mode(doc[key].get<std::string>())) {
    target = doc[key].get<std::string>();
  }
}

void load_non_negative_int(const json& doc, const char* key, int& target) {
  if (auto it = doc.find(key); it != doc.end() && it->is_number_integer()) {
    const int value = it->get<int>();
    if (value >= 0) {
      target = value;
    }
  }
}

} // namespace

Config::Config() {
  if (const auto* spec = niminal::find_provider("openrouter")) {
    provider = std::string(spec->name);
    model = std::string(spec->default_model);
    api_url = std::string(spec->endpoint);
  }
}

std::filesystem::path config_path() {
  const char* home = std::getenv("HOME");
  if ((home == nullptr) || ((*home) == 0)) {
    throw std::runtime_error("HOME is not set; cannot load ~/.niminal/config.json");
  }
  return fs::path(home) / ".niminal" / "config.json";
}

std::filesystem::path models_path() {
  return config_path().parent_path() / "models.json";
}

std::vector<ConfiguredModel> load_models() {
  const auto path = models_path();
  std::ifstream in(path);
  if (!in) {
    throw std::runtime_error("cannot read " + path.string());
  }
  json doc;
  try {
    doc = json::parse(in);
  } catch (const json::exception& e) {
    throw std::runtime_error("invalid " + path.string() + ": " + e.what());
  }
  if (!doc.is_object() || !doc.contains("models") || !doc["models"].is_array()) {
    throw std::runtime_error(path.string() + " must contain a models array");
  }
  std::vector<ConfiguredModel> models;
  for (const auto& entry : doc["models"]) {
    if (!entry.is_object()) {
      throw std::runtime_error("each model in " + path.string() + " must be an object");
    }
    auto required = [&](const char* key) -> std::string {
      if (!entry.contains(key) || !entry[key].is_string() ||
          entry[key].get<std::string>().empty()) {
        throw std::runtime_error(std::string("model ") + key + " is required in " + path.string());
      }
      return entry[key].get<std::string>();
    };
    ConfiguredModel model;
    model.provider = required("provider");
    if (model.provider != "local" && model.provider != "foundry") {
      throw std::runtime_error("unsupported model provider '" + model.provider + "'");
    }
    model.name = required("name");
    model.model = required("model");
    model.api_url = required("api_url");
    if (model.provider == "local") {
      model.runtime = required("runtime");
      if (!supported_local_runtime(model.runtime)) {
        throw std::runtime_error("unsupported local runtime '" + model.runtime + "'");
      }
    } else if (entry.contains("runtime")) {
      throw std::runtime_error("runtime is only valid for local models in " + path.string());
    }
    if (entry.contains("context_window")) {
      if (!entry["context_window"].is_number_integer() || entry["context_window"].get<int>() <= 0) {
        throw std::runtime_error("model context_window must be positive in " + path.string());
      }
      model.context_window = entry["context_window"].get<int>();
    } else if (model.provider == "local") {
      throw std::runtime_error("model context_window must be positive in " + path.string());
    }
    if (std::any_of(models.begin(), models.end(), [&](const ConfiguredModel& other) {
          return other.provider == model.provider && other.name == model.name;
        })) {
      throw std::runtime_error("duplicate " + model.provider + " model name '" + model.name + "'");
    }
    models.push_back(std::move(model));
  }
  return models;
}

std::vector<ConfiguredModel> models_for(std::string_view provider) {
  std::vector<ConfiguredModel> selected;
  for (auto& model : load_models()) {
    if (model.provider == provider) {
      selected.push_back(std::move(model));
    }
  }
  return selected;
}

Config load_config_file(const fs::path& path) {
  Config cfg;
  std::error_code ec;
  if (!fs::exists(path, ec)) {
    return cfg;
  }
  std::ifstream in(path);
  if (!in) {
    return cfg;
  }
  try {
    auto doc = json::parse(in);
    if (doc.contains("model") && doc["model"].is_string()) {
      auto model = doc["model"].get<std::string>();
      if (!model.empty()) {
        cfg.model = std::move(model);
      }
    }
    if (doc.contains("provider") && doc["provider"].is_string()) {
      auto provider = doc["provider"].get<std::string>();
      if (!provider.empty()) {
        cfg.provider = std::move(provider);
      }
    }
    if (doc.contains("thinking") && doc["thinking"].is_string()) {
      cfg.thinking = doc["thinking"].get<std::string>();
    }
    if (doc.contains("show_thinking") && doc["show_thinking"].is_boolean()) {
      cfg.show_thinking = doc["show_thinking"].get<bool>();
    }
    if (doc.contains("theme") && doc["theme"].is_string()) {
      cfg.theme = doc["theme"].get<std::string>();
    }
    if (doc.contains("editor") && doc["editor"].is_string()) {
      auto editor = doc["editor"].get<std::string>();
      if (!editor.empty()) {
        cfg.editor = std::move(editor);
      }
    }
    load_queue_mode(doc, "steering_mode", cfg.steering_mode);
    load_queue_mode(doc, "follow_up_mode", cfg.follow_up_mode);
    load_non_negative_int(doc, "max_steps", cfg.max_steps);
    if (doc.contains("compaction_enabled") && doc["compaction_enabled"].is_boolean()) {
      cfg.compaction_enabled = doc["compaction_enabled"].get<bool>();
    }
    load_non_negative_int(doc, "reserve_tokens", cfg.reserve_tokens);
    load_non_negative_int(doc, "keep_recent_tokens", cfg.keep_recent_tokens);
    load_non_negative_int(doc, "context_window", cfg.context_window);
    if (doc.contains("providers") && doc["providers"].is_object()) {
      for (auto& [name, block] : doc["providers"].items()) {
        if (block.is_object()) {
          if (block.contains("last_model") && block["last_model"].is_string()) {
            auto model = block["last_model"].get<std::string>();
            if (!model.empty()) {
              cfg.last_models[name] = std::move(model);
            }
          }
          if (name != "foundry" && block.contains("api_url") && block["api_url"].is_string()) {
            auto url = block["api_url"].get<std::string>();
            if (!url.empty()) {
              cfg.provider_api_urls[name] = std::move(url);
            }
          }
        }
      }
    }
    if (auto it = cfg.provider_api_urls.find(cfg.provider); it != cfg.provider_api_urls.end()) {
      cfg.api_url = it->second;
    } else if (const auto* spec = niminal::find_provider(cfg.provider)) {
      cfg.api_url = spec->endpoint;
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
  json doc = {{"provider", cfg.provider},           {"model", cfg.model},
              {"show_thinking", cfg.show_thinking}, {"theme", cfg.theme},
              {"steering_mode", cfg.steering_mode}, {"follow_up_mode", cfg.follow_up_mode}};
  if (cfg.max_steps > 0) {
    doc["max_steps"] = cfg.max_steps;
  }
  if (!cfg.compaction_enabled) {
    doc["compaction_enabled"] = false;
  }
  if (cfg.reserve_tokens != kDefaultReserveTokens) {
    doc["reserve_tokens"] = cfg.reserve_tokens;
  }
  if (cfg.keep_recent_tokens != kDefaultKeepRecentTokens) {
    doc["keep_recent_tokens"] = cfg.keep_recent_tokens;
  }
  if (cfg.context_window > 0) {
    doc["context_window"] = cfg.context_window;
  }
  if (!cfg.thinking.empty()) {
    doc["thinking"] = cfg.thinking;
  }
  if (!cfg.editor.empty()) {
    doc["editor"] = cfg.editor;
  }
  if (!cfg.last_models.empty() || !cfg.provider_api_urls.empty()) {
    json providers = json::object();
    for (const auto& [name, url] : cfg.provider_api_urls) {
      if (name != "foundry") {
        providers[name]["api_url"] = url;
      }
    }
    for (const auto& [name, model] : cfg.last_models) {
      providers[name]["last_model"] = model;
    }
    doc["providers"] = std::move(providers);
  }
  auto tmp = path;
  tmp += ".tmp";
  {
    std::ofstream out(tmp, std::ios::binary | std::ios::trunc);
    if (!out) {
      throw std::runtime_error("cannot write " + path.string());
    }
    out << doc.dump(2) << '\n';
  }
  fs::rename(tmp, path);
}

void save_config(const Config& cfg) {
  save_config_file(config_path(), cfg);
}

} // namespace niminal::app
