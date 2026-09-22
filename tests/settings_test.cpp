#include "settings.hpp"

#include "config.hpp"
#include "provider.hpp"

#include <iostream>
#include <string>

namespace {

int fail(const char* msg) {
  std::cerr << msg << '\n';
  return 1;
}

} // namespace

int main() {
  using niminal::app::all_settings;
  using niminal::app::apply_setting_value;
  using niminal::app::Config;
  using niminal::app::cycle_setting;
  using niminal::app::edit_initial_value;
  using niminal::app::format_setting_value;
  using niminal::app::SettingField;
  using niminal::app::SettingKind;
  using niminal::app::toggle_setting;

  if (all_settings().size() != 14) {
    return fail("setting count");
  }
  if (all_settings().front().field != SettingField::provider ||
      all_settings().front().kind != SettingKind::cycle) {
    return fail("first setting");
  }

  Config cfg;
  if (format_setting_value(cfg, SettingField::max_steps, "openrouter", "openai/gpt-4o-mini") !=
      "unlimited") {
    return fail("max_steps display");
  }
  if (format_setting_value(cfg, SettingField::context_window, "openrouter", "openai/gpt-4o-mini")
          .find("128000") == std::string::npos) {
    return fail("context_window display");
  }
  if (format_setting_value(cfg, SettingField::thinking, "openrouter", "openai/gpt-4o-mini") !=
      "(provider default)") {
    return fail("thinking display");
  }

  cfg.max_steps = 5;
  if (edit_initial_value(cfg, SettingField::max_steps) != "5") {
    return fail("edit initial max_steps");
  }

  if (auto result = apply_setting_value(cfg, SettingField::max_steps, "-1", "openrouter",
                                        "openai/gpt-4o-mini");
      result.error.empty()) {
    return fail("reject negative max_steps");
  }
  if (auto result = apply_setting_value(cfg, SettingField::max_steps, "12", "openrouter",
                                        "openai/gpt-4o-mini");
      !result.error.empty() || cfg.max_steps != 12) {
    return fail("apply max_steps");
  }

  cfg.show_thinking = false;
  if (auto result = toggle_setting(cfg, SettingField::show_thinking);
      !result.error.empty() || !cfg.show_thinking) {
    return fail("toggle show_thinking");
  }

  const auto before_provider = cfg.provider;
  const auto initial_model = cfg.model;
  if (auto result = cycle_setting(cfg, SettingField::provider, 1, before_provider, cfg.model);
      !result.error.empty() || !result.agent_changed || cfg.provider == before_provider) {
    return fail("cycle provider");
  }
  const auto switched_provider = cfg.provider;
  if (cfg.last_models.find(before_provider) == cfg.last_models.end() ||
      cfg.last_models[before_provider] != initial_model) {
    return fail("provider switch saves previous model");
  }
  cfg.model = "custom-model";
  if (auto result = cycle_setting(cfg, SettingField::provider, -1, switched_provider, cfg.model);
      !result.error.empty() || cfg.provider != before_provider) {
    return fail("cycle provider back");
  }
  if (cfg.last_models.find(switched_provider) == cfg.last_models.end() ||
      cfg.last_models[switched_provider] != "custom-model") {
    return fail("provider switch updates last_models");
  }

  cfg.thinking.clear();
  if (auto result = cycle_setting(cfg, SettingField::thinking, 1, cfg.provider, cfg.model);
      !result.error.empty() || !result.agent_changed) {
    return fail("cycle thinking");
  }
  if (auto result = cycle_setting(cfg, SettingField::thinking, -1, cfg.provider, cfg.model);
      !result.error.empty() || !cfg.thinking.empty()) {
    return fail("cycle thinking to default");
  }

  cfg.theme = "auto";
  if (auto result = cycle_setting(cfg, SettingField::theme, 1, cfg.provider, cfg.model);
      !result.error.empty() || !result.theme_changed || cfg.theme != "light") {
    return fail("cycle theme");
  }

  cfg.steering_mode = "one-at-a-time";
  if (auto result = cycle_setting(cfg, SettingField::steering_mode, 1, cfg.provider, cfg.model);
      !result.error.empty() || cfg.steering_mode != "all") {
    return fail("cycle steering_mode");
  }

  if (auto result = apply_setting_value(cfg, SettingField::model, "anthropic/claude-sonnet-4-6",
                                        cfg.provider, cfg.model);
      !result.error.empty() || !result.agent_changed ||
      cfg.last_models[cfg.provider] != "anthropic/claude-sonnet-4-6") {
    return fail("apply model");
  }

  std::cout << "settings ok\n";
  return 0;
}
