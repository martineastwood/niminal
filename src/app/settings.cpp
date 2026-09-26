#include "settings.hpp"

#include "provider.hpp"
#include "theme.hpp"
#include "thinking.hpp"

#include <niminal/text.hpp>

#include <algorithm>
#include <optional>
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
  const auto trimmed = niminal::trim_copy(std::string(value));
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

SettingApplyResult apply_int_field(int& field, std::string_view value, const char* label) {
  const auto parsed = parse_non_negative_int(value);
  if (!parsed) {
    return fail(std::string(label) + " must be a non-negative integer");
  }
  field = *parsed;
  return ok();
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

std::string format_setting_value(const Config& cfg, SettingField field) {
  switch (field) {
  case SettingField::provider:
    return cfg.provider;
  case SettingField::model:
    return cfg.model;
  case SettingField::api_url:
    return cfg.api_url.empty() ? "(provider default)" : cfg.api_url;
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
    const auto next = cycle_option(provider_names(), cfg.provider, direction);
    if (auto result = select_provider(cfg, next); !result) {
      return fail(result.error().what());
    }
    return ok(true);
  }
  case SettingField::thinking: {
    const auto next =
        cycle_option(thinking_options(agent_provider, agent_model), cfg.thinking, direction);
    cfg.thinking = next;
    return ok(true);
  }
  case SettingField::theme: {
    const auto next = cycle_option(theme_names(), cfg.theme, direction);
    if (auto theme = load_theme(next); !theme) {
      return fail(theme.error());
    }
    cfg.theme = next;
    return ok(false, true);
  }
  case SettingField::steering_mode:
  case SettingField::follow_up_mode: {
    auto& mode = field == SettingField::steering_mode ? cfg.steering_mode : cfg.follow_up_mode;
    mode = cycle_option(queue_mode_options(), mode, direction);
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

SettingApplyResult apply_setting_value(Config& cfg, SettingField field, std::string_view value) {
  switch (field) {
  case SettingField::model: {
    const auto trimmed = niminal::trim_copy(std::string(value));
    if (trimmed.empty()) {
      return fail("model cannot be empty");
    }
    cfg.model = trimmed;
    cfg.last_models[cfg.provider] = trimmed;
    return ok(true);
  }
  case SettingField::api_url: {
    if (cfg.provider == "local" || cfg.provider == "foundry") {
      return fail("edit this provider's API URL in ~/.niminal/models.json");
    }
    const auto trimmed = niminal::trim_copy(std::string(value));
    cfg.api_url = trimmed;
    if (trimmed.empty()) {
      cfg.provider_api_urls.erase(cfg.provider);
    } else {
      cfg.provider_api_urls[cfg.provider] = trimmed;
    }
    return ok(true);
  }
  case SettingField::editor:
    cfg.editor = niminal::trim_copy(std::string(value));
    return ok();
  case SettingField::max_steps:
    return apply_int_field(cfg.max_steps, value, "max_steps");
  case SettingField::reserve_tokens:
    return apply_int_field(cfg.reserve_tokens, value, "reserve_tokens");
  case SettingField::keep_recent_tokens:
    return apply_int_field(cfg.keep_recent_tokens, value, "keep_recent_tokens");
  case SettingField::context_window:
    return apply_int_field(cfg.context_window, value, "context_window");
  default:
    return fail("field is not editable");
  }
}

} // namespace niminal::app
