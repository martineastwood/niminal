#include "thinking.hpp"
#include "models_dev.hpp"

#include <algorithm>
#include <stdexcept>

namespace niminal::app {
namespace {

using json = nlohmann::json;

std::string lower(std::string s) {
  for (char& c : s)
    if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
  return s;
}

bool starts_family(std::string_view model, std::string_view family) {
  if (model == family) return true;
  if (model.size() <= family.size()) return false;
  return model.substr(0, family.size()) == family && model[family.size()] == '-';
}

int level_index(std::string_view s) {
  for (int i = 0; i < 7; ++i)
    if (kThinkingLevels[i] == s) return i;
  return -1;
}

std::vector<std::string> anthropic_efforts(std::string_view model) {
  auto m = lower(std::string(model));
  for (const char* family : {"claude-sonnet-4-6", "claude-opus-4-6"})
    if (starts_family(m, family)) return {"low", "medium", "high", "max"};
  for (const char* family : {"claude-opus-4-7", "claude-opus-4-8", "claude-opus-5",
                             "claude-sonnet-5", "claude-fable-5"})
    if (starts_family(m, family))
      return {"low", "medium", "high", "xhigh", "max"};
  return {};
}

int budget_tokens(std::string_view level) {
  if (level == "minimal") return 1024;
  if (level == "low") return 2048;
  if (level == "medium") return 8000;
  if (level == "high") return 16000;
  if (level == "xhigh" || level == "max") return 32000;
  return 0;
}

json effort_options(std::string_view provider, std::string_view level) {
  json out = json::object();
  auto p = lower(std::string(provider));
  auto lv = std::string(level);
  if (p == "openrouter" || p == "openai" || p == "hyper")
    out["reasoning"] = json{{"effort", lv}};
  else if (p == "mistral" || p == "opencode" || p == "opencodezen" ||
           p == "google")
    out["reasoning_effort"] = lv;
  else if (p == "anthropic") {
    auto budget = budget_tokens(lv);
    if (budget > 0)
      out["thinking"] = json{{"type", "enabled"}, {"budget_tokens", budget}};
  }
  return out;
}

json toggle_options(std::string_view provider) {
  auto p = lower(std::string(provider));
  if (p == "openrouter") return json{{"reasoning", json{{"enabled", true}}}};
  if (p == "openai" || p == "hyper")
    return json{{"reasoning", json{{"effort", "medium"}}}};
  if (p == "mistral" || p == "opencode" || p == "opencodezen")
    return json{{"reasoning_effort", "medium"}};
  if (p == "google") return effort_options(p, "high");
  if (p == "anthropic") return effort_options(p, "high");
  return json::object();
}

json max_token_options(std::string_view provider, std::string_view level) {
  auto p = lower(std::string(provider));
  if (p == "openrouter")
    return json{{"reasoning", json{{"max_tokens", budget_tokens(level)}}}};
  return effort_options(p, level);
}

json anthropic_adaptive(std::string_view model, std::string_view level) {
  auto efforts = anthropic_efforts(model);
  auto lv = std::string(level);
  if (lv == "minimal") lv = "low";
  else if (lv == "xhigh" &&
           std::find(efforts.begin(), efforts.end(), "xhigh") == efforts.end())
    lv = "max";
  return json{{"thinking", json{{"type", "adaptive"}, {"display", "summarized"}}},
              {"output_config", json{{"effort", lv}}}};
}

struct Plan {
  std::string label;
  json options = json::object();
};

Plan resolve(std::string_view provider, std::string_view model,
             std::string_view want) {
  Plan plan;
  if (want.empty()) return plan;
  auto p = lower(std::string(provider));
  auto m = std::string(model);
  auto efforts = anthropic_efforts(m);
  if (p == "anthropic" && !efforts.empty()) {
    bool required = want == "none" && starts_family(lower(m), "claude-fable-5");
    auto level = required ? "low" : std::string(want);
    if (level == "none") {
      plan.label = "off";
      plan.options = json{{"thinking", json{{"type", "disabled"}}}};
      return plan;
    }
    plan.options = anthropic_adaptive(m, level);
    plan.label = required ? "low (thinking required)"
                          : plan.options["output_config"]["effort"].get<std::string>();
    return plan;
  }
  auto caps = lookup_reasoning_caps(provider, model);
  if (caps.known && !caps.reasoning) return plan;
  if (caps.known && !caps.efforts.empty()) {
    auto snapped = snap_to_efforts(want, caps.efforts);
    if (snapped.empty() || snapped == "none") {
      plan.label = "off";
      return plan;
    }
    plan.label = snapped;
    plan.options = effort_options(p, snapped);
    return plan;
  }
  if (caps.known && caps.toggle) {
    if (want == "none") {
      plan.label = "off";
      return plan;
    }
    plan.label = "on";
    plan.options = toggle_options(p);
    return plan;
  }
  if (caps.known && caps.budget_tokens) {
    if (want == "none") {
      plan.label = "off";
      return plan;
    }
    plan.label = std::string(want);
    plan.options = max_token_options(p, want);
    return plan;
  }
  if (caps.known) return plan;
  if (want == "none") {
    plan.label = "off";
    return plan;
  }
  plan.label = std::string(want);
  plan.options = effort_options(p, want);
  return plan;
}

}  // namespace

using json = nlohmann::json;

std::string thinking_levels_help() {
  std::string out;
  for (int i = 0; i < 7; ++i) {
    if (i) out += '|';
    out += kThinkingLevels[i];
  }
  return out;
}

std::string normalize_thinking(std::string_view value) {
  auto v = lower(std::string(value));
  while (!v.empty() && (v.front() == ' ' || v.front() == '\t')) v.erase(v.begin());
  while (!v.empty() && (v.back() == ' ' || v.back() == '\t')) v.pop_back();
  if (v.empty()) return {};
  for (auto* level : kThinkingLevels)
    if (v == level) return v;
  throw std::runtime_error("invalid thinking level '" + std::string(value) +
                           "' (use " + thinking_levels_help() + ")");
}

std::string snap_to_efforts(std::string_view want,
                            const std::vector<std::string>& efforts) {
  if (want.empty()) return {};
  for (const auto& e : efforts)
    if (e == want) return std::string(want);
  if (want == "none") return {};
  int wi = level_index(want);
  if (wi < 0) return {};
  int best_i = -1;
  int best_d = 100;
  std::string best;
  for (const auto& e : efforts) {
    int ei = level_index(e);
    if (ei < 0) continue;
    int d = ei > wi ? ei - wi : wi - ei;
    if (best_i < 0 || d < best_d || (d == best_d && ei > best_i)) {
      best_d = d;
      best_i = ei;
      best = e;
    }
  }
  return best;
}

std::vector<std::string> thinking_choices(std::string_view provider,
                                          std::string_view model) {
  auto p = lower(std::string(provider));
  if (p == "anthropic") {
    auto efforts = anthropic_efforts(model);
    if (!efforts.empty()) {
      if (starts_family(lower(std::string(model)), "claude-fable-5")) return efforts;
      std::vector<std::string> out{"none"};
      out.insert(out.end(), efforts.begin(), efforts.end());
      return out;
    }
  }
  auto caps = lookup_reasoning_caps(provider, model);
  if (!caps.known) {
    std::vector<std::string> out;
    for (auto* level : kThinkingLevels) out.emplace_back(level);
    return out;
  }
  if (!caps.reasoning) return {};
  std::vector<std::string> out{"none"};
  if (!caps.efforts.empty()) {
    for (auto* level : kThinkingLevels) {
      if (std::string_view(level) == "none") continue;
      if (std::find(caps.efforts.begin(), caps.efforts.end(), level) !=
          caps.efforts.end())
        out.emplace_back(level);
    }
    return out;
  }
  if (caps.toggle) {
    out.emplace_back("high");
    return out;
  }
  if (caps.budget_tokens) {
    for (auto* level : kThinkingLevels)
      if (std::string_view(level) != "none") out.emplace_back(level);
  }
  return out;
}

std::string thinking_status(std::string_view provider, std::string_view model,
                            std::string_view want) {
  std::string level;
  try {
    level = want.empty() ? "" : normalize_thinking(want);
  } catch (...) {
    return {};
  }
  return resolve(provider, model, level).label;
}

json thinking_body(std::string_view provider, std::string_view model,
                   std::string_view want) {
  std::string level;
  try {
    level = want.empty() ? "" : normalize_thinking(want);
  } catch (...) {
    return json::object();
  }
  return resolve(provider, model, level).options;
}

}  // namespace niminal::app
