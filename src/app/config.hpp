#pragma once

#include <filesystem>
#include <map>
#include <string>

namespace niminal::app {

constexpr int kDefaultContextWindow = 128'000;
constexpr int kDefaultReserveTokens = 16'384;
constexpr int kDefaultKeepRecentTokens = 20'000;
constexpr int kSummaryMaxTokens = 4'096;
constexpr int kDefaultThinkingPreviewChars = 360;
constexpr int kDefaultThinkingPreviewLines = 4;

struct Config {
  Config();

  std::string provider;
  std::string model;
  std::string api_url;
  std::string thinking;
  bool show_thinking = false;
  int thinking_preview_chars = kDefaultThinkingPreviewChars;
  int thinking_preview_lines = kDefaultThinkingPreviewLines;
  std::string theme = "auto";
  std::string steering_mode = "one-at-a-time";
  std::string follow_up_mode = "one-at-a-time";
  int max_steps = 0; // 0 means unlimited.
  bool compaction_enabled = true;
  int reserve_tokens = kDefaultReserveTokens;
  int keep_recent_tokens = kDefaultKeepRecentTokens;
  int context_window = 0; // 0 means use the built-in default.
  std::map<std::string, std::string> last_models;
};

std::filesystem::path config_path();
Config load_config();
Config load_config_file(const std::filesystem::path& path);
void save_config(const Config& cfg);
void save_config_file(const std::filesystem::path& path, const Config& cfg);

} // namespace niminal::app
