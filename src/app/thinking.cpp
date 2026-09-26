#include "thinking.hpp"
#include "models_dev.hpp"

#include <niminal/text.hpp>

#include <algorithm>
#include <stdexcept>
#include <utility>

namespace niminal::app {

using json = nlohmann::json;

namespace {

bool starts_family(std::string_view model, std::string_view family) {
  if (model == family) {
    return true;
  }
  if (model.size() <= family.size()) {
    return false;
  }
  return model.substr(0, family.size()) == family && model[family.size()] == '-';
}

int level_index(std::string_view s) {
  for (int i = 0; i < 7; ++i) {
    if (kThinkingLevels[i] == s) {
      return i;
    }
  }
  return -1;
}

std::vector<std::string> anthropic_efforts(std::string_view model) {
  auto m = niminal::lower_copy(std::string(model));
  for (const char* family : {"claude-sonnet-4-6", "claude-opus-4-6"}) {
    if (starts_family(m, family)) {
      return {"low", "medium", "high", "max"};
    }
  }
  for (const char* family : {"claude-opus-4-7", "claude-opus-4-8", "claude-opus-5",
                             "claude-sonnet-5", "claude-fable-5"}) {
    if (starts_family(m, family)) {
      return {"low", "medium", "high", "xhigh", "max"};
    }
  }
  return {};
}

int budget_tokens(std::string_view level) {
  constexpr std::pair<std::string_view, int> budgets[] = {{"minimal", 1024}, {"low", 2048},
                                                          {"medium", 8000},  {"high", 16000},
                                                          {"xhigh", 32000},  {"max", 32000}};
  for (const auto& [name, tokens] : budgets) {
    if (level == name) {
      return tokens;
    }
  }
  return 0;
}

json effort_options(std::string_view provider, std::string_view level) {
  json out = json::object();
  auto p = niminal::lower_copy(std::string(provider));
  auto lv = std::string(level);
  if (p == "openrouter" || p == "openai" || p == "foundry" || p == "hyper") {
    out["reasoning"] = json{{"effort", lv}};
  } else if (p == "mistral" || p == "opencode" || p == "opencodezen" || p == "google") {
    out["reasoning_effort"] = lv;
  } else if (p == "anthropic") {
    auto budget = budget_tokens(lv);
    if (budget > 0) {
      out["thinking"] = json{{"type", "enabled"}, {"budget_tokens", budget}};
    }
  }
  return out;
}

json toggle_options(std::string_view provider) {
  auto p = niminal::lower_copy(std::string(provider));
  if (p == "openrouter") {
    return json{{"reasoning", json{{"enabled", true}}}};
  }
  if (p == "openai" || p == "foundry" || p == "hyper") {
    return json{{"reasoning", json{{"effort", "medium"}}}};
  }
  if (p == "mistral" || p == "opencode" || p == "opencodezen") {
    return json{{"reasoning_effort", "medium"}};
  }
  if (p == "google" || p == "anthropic") {
    return effort_options(p, "high");
  }
  return json::object();
}

json max_token_options(std::string_view provider, std::string_view level) {
  auto p = niminal::lower_copy(std::string(provider));
  if (p == "openrouter") {
    return json{{"reasoning", json{{"max_tokens", budget_tokens(level)}}}};
  }
  return effort_options(p, level);
}

json anthropic_adaptive(std::string_view model, std::string_view level) {
  auto efforts = anthropic_efforts(model);
  auto lv = std::string(level);
  if (lv == "minimal") {
    lv = "low";
  } else if (lv == "xhigh" && std::find(efforts.begin(), efforts.end(), "xhigh") == efforts.end()) {
    lv = "max";
  }
  return json{{"thinking", json{{"type", "adaptive"}, {"display", "summarized"}}},
              {"output_config", json{{"effort", lv}}}};
}

struct Plan {
  std::string label;
  json options = json::object();
};

Plan resolve(std::string_view provider, std::string_view model, std::string_view want) {
  if (want.empty()) {
    return {};
  }
  auto p = niminal::lower_copy(std::string(provider));
  auto m = std::string(model);
  auto efforts = anthropic_efforts(m);
  if (p == "anthropic" && !efforts.empty()) {
    bool required = want == "none" && starts_family(niminal::lower_copy(m), "claude-fable-5");
    auto level = required ? "low" : std::string(want);
    if (level == "none") {
      return {"off", json{{"thinking", json{{"type", "disabled"}}}}};
    }
    auto options = anthropic_adaptive(m, level);
    auto label = required ? "low (thinking required)"
                          : options["output_config"]["effort"].get<std::string>();
    return {label, std::move(options)};
  }
  auto caps = lookup_reasoning_caps(provider, model);
  if (caps.known && !caps.reasoning) {
    return {};
  }
  if (!caps.known) {
    if (want == "none") {
      return {"off"};
    }
    return {std::string(want), effort_options(p, want)};
  }
  if (!caps.efforts.empty()) {
    auto snapped = snap_to_efforts(want, caps.efforts);
    if (snapped.empty() || snapped == "none") {
      return {"off"};
    }
    return {snapped, effort_options(p, snapped)};
  }
  if (want == "none") {
    return {"off"};
  }
  if (caps.toggle) {
    return {"on", toggle_options(p)};
  }
  if (caps.budget_tokens) {
    return {std::string(want), max_token_options(p, want)};
  }
  return {};
}

std::string thinking_levels_help() {
  std::string out = kThinkingLevels[0];
  for (int i = 1; i < 7; ++i) {
    out += '|' + std::string(kThinkingLevels[i]);
  }
  return out;
}

std::string normalized_or_empty(std::string_view value) {
  try {
    return value.empty() ? std::string() : normalize_thinking(value);
  } catch (...) {
    return {};
  }
}

} // namespace

std::string normalize_thinking(std::string_view value) {
  auto v = niminal::lower_copy(std::string(value));
  while (!v.empty() && (v.front() == ' ' || v.front() == '\t')) {
    v.erase(v.begin());
  }
  while (!v.empty() && (v.back() == ' ' || v.back() == '\t')) {
    v.pop_back();
  }
  if (v.empty()) {
    return {};
  }
  for (auto* level : kThinkingLevels) {
    if (v == level) {
      return v;
    }
  }
  throw std::runtime_error("invalid thinking level '" + std::string(value) + "' (use " +
                           thinking_levels_help() + ")");
}

std::string snap_to_efforts(std::string_view want, const std::vector<std::string>& efforts) {
  if (want.empty()) {
    return {};
  }
  for (const auto& e : efforts) {
    if (e == want) {
      return std::string(want);
    }
  }
  if (want == "none") {
    return {};
  }
  int wi = level_index(want);
  if (wi < 0) {
    return {};
  }
  int best_i = -1;
  int best_d = 100;
  std::string best;
  for (const auto& e : efforts) {
    int ei = level_index(e);
    if (ei < 0) {
      continue;
    }
    int d = ei > wi ? ei - wi : wi - ei;
    if (best_i < 0 || d < best_d || (d == best_d && ei > best_i)) {
      best_d = d;
      best_i = ei;
      best = e;
    }
  }
  return best;
}

std::vector<std::string> thinking_choices(std::string_view provider, std::string_view model) {
  auto p = niminal::lower_copy(std::string(provider));
  if (p == "anthropic") {
    auto efforts = anthropic_efforts(model);
    if (!efforts.empty()) {
      if (starts_family(niminal::lower_copy(std::string(model)), "claude-fable-5")) {
        return efforts;
      }
      std::vector<std::string> out{"none"};
      out.insert(out.end(), efforts.begin(), efforts.end());
      return out;
    }
  }
  auto caps = lookup_reasoning_caps(provider, model);
  if (!caps.known) {
    std::vector<std::string> out;
    for (auto* level : kThinkingLevels) {
      out.emplace_back(level);
    }
    return out;
  }
  if (!caps.reasoning) {
    return {};
  }
  std::vector<std::string> out{"none"};
  if (!caps.efforts.empty()) {
    for (auto* level : kThinkingLevels) {
      if (std::string_view(level) == "none") {
        continue;
      }
      if (std::find(caps.efforts.begin(), caps.efforts.end(), level) != caps.efforts.end()) {
        out.emplace_back(level);
      }
    }
    return out;
  }
  if (caps.toggle) {
    out.emplace_back("high");
    return out;
  }
  if (caps.budget_tokens) {
    for (auto* level : kThinkingLevels) {
      if (std::string_view(level) != "none") {
        out.emplace_back(level);
      }
    }
  }
  return out;
}

std::string thinking_status(std::string_view provider, std::string_view model,
                            std::string_view want) {
  return resolve(provider, model, normalized_or_empty(want)).label;
}

json thinking_body(std::string_view provider, std::string_view model, std::string_view want) {
  return resolve(provider, model, normalized_or_empty(want)).options;
}

} // namespace niminal::app
