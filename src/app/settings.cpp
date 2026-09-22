#include "settings.hpp"

#include "provider.hpp"
#include "queue_mode.hpp"
#include "theme.hpp"
#include "thinking.hpp"

#include <niminal/providers.hpp>
#include <niminal/text.hpp>

#include <algorithm>
#include <cctype>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

namespace niminal::app {
namespace {

constexpr SettingSpec kSettings[] = {
    {SettingField::provider, "provider", SettingKind::cycle},
    {SettingField::model, "model", SettingKind::text},
    {SettingField::api_url, "api_url", SettingKind::text},
    {SettingField::thinking, "thinking", SettingKind::cycle},
    {SettingField::show_thinking, "show_thinking", SettingKind::toggle},
    {SettingField::theme, "theme", SettingKind::cycle},
    {SettingField::editor, "editor", SettingKind::text},
    {SettingField::steering_mode, "steering_mode", SettingKind::cycle},
    {SettingField::follow_up_mode, "follow_up_mode", SettingKind::cycle},
    {SettingField::max_steps, "max_steps", SettingKind::integer},
    {SettingField::compaction_enabled, "compaction_enabled", SettingKind::toggle},
    {SettingField::reserve_tokens, "reserve_tokens", SettingKind::integer},
    {SettingField::keep_recent_tokens, "keep_recent_tokens", SettingKind::integer},
    {SettingField::context_window, "context_window", SettingKind::integer},
};

std::string trim_copy(std::string_view value) {
  std::string s(value);
  while (!s.empty() &&
         (s.back() == ' ' || s.back() == '\n' || s.back() == '\r' || s.back() == '\t')) {
    s.pop_back();
  }
  size_t i = 0;
  while (i < s.size() && (s[i] == ' ' || s[i] == '\n' || s[i] == '\r' || s[i] == '\t')) {
    ++i;
  }
  return s.substr(i);
}

std::vector<std::string> provider_options() {
  std::vector<std::string> out;
  for (const auto& spec : niminal::all_providers()) {
    out.emplace_back(spec.name);
  }
  return out;
}

std::vector<std::string> thinking_options(std::string_view provider, std::string_view model) {
  std::vector<std::string> out{""};
  for (const auto& choice : thinking_choices(provider, model)) {
    if (choice == "none") {
      continue;
    }
    if (std::find(out.begin(), out.end(), choice) == out.end()) {
      out.push_back(choice);
    }
  }
  return out;
}

std::vector<std::string> theme_options() {
  return {theme_mode_name(ThemeMode::automatic), theme_mode_name(ThemeMode::light),
          theme_mode_name(ThemeMode::dark)};
}

std::vector<std::string> queue_mode_options() {
  return {"one-at-a-time", "all"};
}

int find_option_index(const std::vector<std::string>& options, std::string_view current) {
  for (size_t i = 0; i < options.size(); ++i) {
    if (options[i] == current) {
      return static_cast<int>(i);
    }
  }
  return 0;
}

std::string cycle_option(const std::vector<std::string>& options, std::string_view current,
                         int direction) {
  if (options.empty()) {
    return std::string(current);
  }
  const int index = find_option_index(options, current);
  const int next = (index + direction) % static_cast<int>(options.size());
  const int wrapped = next < 0 ? next + static_cast<int>(options.size()) : next;
  return options[static_cast<size_t>(wrapped)];
}

SettingApplyResult ok(bool agent_changed = false, bool theme_changed = false) {
  return {.error = {}, .agent_changed = agent_changed, .theme_changed = theme_changed};
}

SettingApplyResult fail(std::string message) {
  return {.error = std::move(message)};
}

std::optional<int> parse_non_negative_int(std::string_view value) {
  const auto trimmed = trim_copy(value);
  if (trimmed.empty()) {
    return 0;
  }
  try {
    size_t consumed = 0;
    const int parsed = std::stoi(trimmed, &consumed);
    if (consumed != trimmed.size() || parsed < 0) {
      return std::nullopt;
    }
    return parsed;
  } catch (...) {
    return std::nullopt;
  }
}

SettingApplyResult apply_thinking(Config& cfg, std::string_view value,
                                  std::string_view agent_provider, std::string_view agent_model) {
  const auto trimmed = trim_copy(value);
  if (trimmed.empty()) {
    cfg.thinking.clear();
    return ok(true);
  }
  try {
    cfg.thinking = normalize_thinking(trimmed);
  } catch (const std::exception& e) {
    return fail(e.what());
  }
  const auto options = thinking_options(agent_provider, agent_model);
  if (std::find(options.begin(), options.end(), cfg.thinking) == options.end() &&
      !(cfg.thinking.empty() && std::find(options.begin(), options.end(), "") != options.end())) {
    return fail("thinking level not supported for the current model");
  }
  return ok(true);
}

} // namespace

std::span<const SettingSpec> all_settings() {
  return kSettings;
}

size_t setting_count() {
  return std::size(kSettings);
}

const SettingSpec* setting_at(size_t index) {
  if (index >= setting_count()) {
    return nullptr;
  }
  return &kSettings[index];
}

std::string format_setting_value(const Config& cfg, SettingField field,
                                 std::string_view /*agent_provider*/,
                                 std::string_view /*agent_model*/) {
  switch (field) {
  case SettingField::provider:
    return cfg.provider;
  case SettingField::model:
    return cfg.model;
  case SettingField::api_url:
    return cfg.api_url;
  case SettingField::thinking:
    if (cfg.thinking.empty()) {
      return "(provider default)";
    }
    return cfg.thinking;
  case SettingField::show_thinking:
    return cfg.show_thinking ? "on" : "off";
  case SettingField::theme:
    return cfg.theme;
  case SettingField::editor:
    return cfg.editor.empty() ? "(unset)" : cfg.editor;
  case SettingField::steering_mode:
    return cfg.steering_mode;
  case SettingField::follow_up_mode:
    return cfg.follow_up_mode;
  case SettingField::max_steps:
    return cfg.max_steps == 0 ? "unlimited" : std::to_string(cfg.max_steps);
  case SettingField::compaction_enabled:
    return cfg.compaction_enabled ? "on" : "off";
  case SettingField::reserve_tokens:
    return std::to_string(cfg.reserve_tokens);
  case SettingField::keep_recent_tokens:
    return std::to_string(cfg.keep_recent_tokens);
  case SettingField::context_window:
    return cfg.context_window == 0 ? std::to_string(kDefaultContextWindow) + " (default)"
                                   : std::to_string(cfg.context_window);
  }
  return {};
}

std::string edit_initial_value(const Config& cfg, SettingField field) {
  switch (field) {
  case SettingField::model:
    return cfg.model;
  case SettingField::api_url:
    return cfg.api_url;
  case SettingField::editor:
    return cfg.editor;
  case SettingField::max_steps:
    return std::to_string(cfg.max_steps);
  case SettingField::reserve_tokens:
    return std::to_string(cfg.reserve_tokens);
  case SettingField::keep_recent_tokens:
    return std::to_string(cfg.keep_recent_tokens);
  case SettingField::context_window:
    return cfg.context_window == 0 ? "0" : std::to_string(cfg.context_window);
  default:
    return {};
  }
}

SettingApplyResult cycle_setting(Config& cfg, SettingField field, int direction,
                                 std::string_view agent_provider, std::string_view agent_model) {
  switch (field) {
  case SettingField::provider: {
    const auto next = cycle_option(provider_options(), cfg.provider, direction);
    if (auto result = select_provider(cfg, next); !result) {
      return fail(result.error().what());
    }
    return ok(true);
  }
  case SettingField::thinking: {
    const auto next =
        cycle_option(thinking_options(agent_provider, agent_model), cfg.thinking, direction);
    return apply_thinking(cfg, next, agent_provider, agent_model);
  }
  case SettingField::theme: {
    const auto next = cycle_option(theme_options(), cfg.theme, direction);
    const auto mode = parse_theme_mode(next);
    if (!mode) {
      return fail("invalid theme");
    }
    cfg.theme = theme_mode_name(*mode);
    return ok(false, true);
  }
  case SettingField::steering_mode: {
    const auto next = cycle_option(queue_mode_options(), cfg.steering_mode, direction);
    if (!valid_queue_mode(next)) {
      return fail("invalid steering_mode");
    }
    cfg.steering_mode = next;
    return ok();
  }
  case SettingField::follow_up_mode: {
    const auto next = cycle_option(queue_mode_options(), cfg.follow_up_mode, direction);
    if (!valid_queue_mode(next)) {
      return fail("invalid follow_up_mode");
    }
    cfg.follow_up_mode = next;
    return ok();
  }
  default:
    return fail("field is not cyclable");
  }
}

SettingApplyResult toggle_setting(Config& cfg, SettingField field) {
  switch (field) {
  case SettingField::show_thinking:
    cfg.show_thinking = !cfg.show_thinking;
    return ok();
  case SettingField::compaction_enabled:
    cfg.compaction_enabled = !cfg.compaction_enabled;
    return ok();
  default:
    return fail("field is not toggleable");
  }
}

SettingApplyResult apply_setting_value(Config& cfg, SettingField field, std::string_view value,
                                       std::string_view /*agent_provider*/,
                                       std::string_view /*agent_model*/) {
  switch (field) {
  case SettingField::model: {
    const auto trimmed = trim_copy(value);
    if (trimmed.empty()) {
      return fail("model cannot be empty");
    }
    cfg.model = trimmed;
    cfg.last_models[cfg.provider] = trimmed;
    return ok(true);
  }
  case SettingField::api_url: {
    const auto trimmed = trim_copy(value);
    if (trimmed.empty()) {
      return fail("api_url cannot be empty");
    }
    cfg.api_url = trimmed;
    return ok(true);
  }
  case SettingField::editor:
    cfg.editor = trim_copy(value);
    return ok();
  case SettingField::max_steps: {
    const auto parsed = parse_non_negative_int(value);
    if (!parsed) {
      return fail("max_steps must be a non-negative integer");
    }
    cfg.max_steps = *parsed;
    return ok();
  }
  case SettingField::reserve_tokens: {
    const auto parsed = parse_non_negative_int(value);
    if (!parsed) {
      return fail("reserve_tokens must be a non-negative integer");
    }
    cfg.reserve_tokens = *parsed;
    return ok();
  }
  case SettingField::keep_recent_tokens: {
    const auto parsed = parse_non_negative_int(value);
    if (!parsed) {
      return fail("keep_recent_tokens must be a non-negative integer");
    }
    cfg.keep_recent_tokens = *parsed;
    return ok();
  }
  case SettingField::context_window: {
    const auto parsed = parse_non_negative_int(value);
    if (!parsed) {
      return fail("context_window must be a non-negative integer");
    }
    cfg.context_window = *parsed;
    return ok();
  }
  default:
    return fail("field is not editable");
  }
}

} // namespace niminal::app
