#pragma once

#include <filesystem>
#include <map>
#include <string>

namespace niminal::app {

struct Config {
  std::string provider = "openrouter";
  std::string model = "openai/gpt-4o-mini";
  std::string api_url = "https://openrouter.ai/api/v1/chat/completions";
  std::string thinking;
  std::string steering_mode = "one-at-a-time";
  std::string follow_up_mode = "one-at-a-time";
  std::map<std::string, std::string> last_models;
};

std::filesystem::path config_path();
Config load_config();
Config load_config_file(const std::filesystem::path& path);
void save_config(const Config& cfg);
void save_config_file(const std::filesystem::path& path, const Config& cfg);

}  // namespace niminal::app
