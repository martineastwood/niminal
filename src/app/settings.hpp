#pragma once

#include "config.hpp"

#include <span>
#include <string>
#include <string_view>

namespace niminal::app {

enum class SettingKind { cycle, toggle, text, integer };

enum class SettingField {
  provider,
  model,
  api_url,
  thinking,
  show_thinking,
  theme,
  editor,
  steering_mode,
  follow_up_mode,
  max_steps,
  compaction_enabled,
  reserve_tokens,
  keep_recent_tokens,
  context_window,
};

struct SettingSpec {
  SettingField field;
  const char* label;
  SettingKind kind;
};

std::span<const SettingSpec> all_settings();
size_t setting_count();
const SettingSpec* setting_at(size_t index);

std::string format_setting_value(const Config& cfg, SettingField field,
                                 std::string_view agent_provider, std::string_view agent_model);
std::string edit_initial_value(const Config& cfg, SettingField field);

struct SettingApplyResult {
  std::string error;
  bool agent_changed = false;
  bool theme_changed = false;
};

SettingApplyResult cycle_setting(Config& cfg, SettingField field, int direction,
                                 std::string_view agent_provider, std::string_view agent_model);
SettingApplyResult toggle_setting(Config& cfg, SettingField field);
SettingApplyResult apply_setting_value(Config& cfg, SettingField field, std::string_view value,
                                       std::string_view agent_provider,
                                       std::string_view agent_model);

} // namespace niminal::app
