#include "modes.hpp"

#include <cassert>
#include <cctype>
#include <ranges>

namespace niminal::app {
namespace {

const char* kActPrompt =
    R"(Current mode: ACT (authoritative). Implement requested changes and verify them.
Reuse the most recent plan and tool results in this session; do not repeat broad
repository exploration unless new evidence or a changed assumption requires it.
For multi-step work, follow the agreed plan when one exists; otherwise use a short
ordered plan. Report meaningful progress and explain deviations as the work evolves.
Skip checklists for simple requests.)";

const char* kPlanPrompt =
    R"(Current mode: PLAN (authoritative). Research the codebase, grill the user until the
design is settled, then write the plan.
Do not modify files, run shell commands, or call tools that change state.
Look up every fact you can in the codebase with read-only inspection tools and git history.
Facts come from the repo; the decisions belong to the user.
When the request is ambiguous or leaves real design choices open, interview the user with
`ask_user`: ask one question at a time and wait for the answer before the next, offer 2 to 4
concrete options with your recommendation first, and take as many rounds as the design needs.
Walk the design tree branch by branch and resolve dependencies between decisions one at a
time. Ask only about choices that change what you would build, and never ask for something
the codebase already answers.
Never mix questions and the plan in the same response.
Skip the interview when the request is already unambiguous. When the decisions are settled,
or the user tells you to stop asking, write a short plan: the files to touch, the steps in
order, and how you will verify the result. Do not start editing.
If `ask_user` is not available in this session, put the open questions in your response.
When the user switches to act mode, they may ask you to implement the plan.)";

ModeSpec builtin_act() {
  return ModeSpec{.id = "act",
                  .label = "Act",
                  .prompt = kActPrompt,
                  .tool_allowlist = {},
                  .profile = ModePermissionProfile::ask};
}

ModeSpec builtin_plan() {
  return ModeSpec{.id = "plan",
                  .label = "Plan",
                  .prompt = kPlanPrompt,
                  .tool_allowlist = {"read", "grep", "glob", "ls", "git", "ask_user"},
                  .profile = ModePermissionProfile::read_only};
}

bool tool_allowed(const std::vector<std::string>& allowed, const std::string& name) {
  if (allowed.empty()) {
    return true;
  }
  const auto normalized = ModeController::normalize_tool_name(name);
  return std::ranges::find(allowed, normalized) != allowed.end();
}

} // namespace

ModeController::ModeController() {
  rebuild_modes();
}

void ModeController::rebuild_modes() {
  modes_.clear();
  modes_.push_back(builtin_act());
  modes_.push_back(builtin_plan());
  for (const auto& mode : extension_modes_) {
    modes_.push_back(mode);
  }
  if (find(current_id_) == nullptr) {
    current_id_ = "act";
  }
}

void ModeController::set_extension_modes(std::vector<ModeSpec> modes) {
  extension_modes_.clear();
  for (auto& mode : modes) {
    if (mode.id.empty() || mode.label.empty() || mode.prompt.empty()) {
      continue;
    }
    if (mode.id == "act" || mode.id == "plan") {
      continue;
    }
    if (std::ranges::any_of(extension_modes_, [&](const ModeSpec& existing) {
          return existing.id == mode.id;
        })) {
      continue;
    }
    extension_modes_.push_back(std::move(mode));
  }
  rebuild_modes();
}

void ModeController::refresh(niminal::Agent& agent, std::function<void(niminal::Agent&)> prepare) {
  if (!catalog_.empty()) {
    agent.tools = catalog_;
  }
  if (prepare) {
    prepare(agent);
  }
  catalog_ = agent.tools;
  apply_tools(agent);
}

void ModeController::apply_tools(niminal::Agent& agent) const {
  if (catalog_.empty()) {
    return;
  }
  const auto& mode = require_current();
  agent.tools.clear();
  for (const auto& tool : catalog_) {
    if (tool_allowed(mode.tool_allowlist, tool.name)) {
      agent.tools.push_back(tool);
    }
  }
}

std::string ModeController::normalize_tool_name(std::string name) {
  const auto first = name.find_first_not_of(" \t\r\n");
  if (first == std::string::npos) {
    return {};
  }
  const auto last = name.find_last_not_of(" \t\r\n");
  name = name.substr(first, last - first + 1);
  for (char& c : name) {
    c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
  }
  return name;
}

std::optional<ModePermissionProfile> ModeController::parse_profile(std::string_view name) {
  if (name == "read_only") {
    return ModePermissionProfile::read_only;
  }
  if (name == "ask") {
    return ModePermissionProfile::ask;
  }
  if (name == "yolo") {
    return ModePermissionProfile::yolo;
  }
  return std::nullopt;
}

const ModeSpec* ModeController::find(std::string_view id) const {
  for (const auto& mode : modes_) {
    if (mode.id == id) {
      return &mode;
    }
  }
  return nullptr;
}

const ModeSpec& ModeController::require_current() const {
  const auto* mode = find(current_id_);
  assert(mode != nullptr);
  return *mode;
}

bool ModeController::set_mode(std::string_view id) {
  if (find(id) == nullptr) {
    return false;
  }
  current_id_ = std::string(id);
  return true;
}

bool ModeController::cycle_next() {
  const auto it = std::ranges::find_if(modes_, [&](const ModeSpec& mode) { return mode.id == current_id_; });
  const size_t index = it == modes_.end() ? 0 : static_cast<size_t>(it - modes_.begin());
  current_id_ = modes_[(index + 1) % modes_.size()].id;
  return true;
}

std::string ModeController::label() const {
  return require_current().label;
}

std::string ModeController::mode_prompt() const {
  return require_current().prompt;
}

ModePermissionProfile ModeController::profile() const {
  return require_current().profile;
}

bool ModeController::skips_tool_approval() const {
  const auto prof = profile();
  return prof == ModePermissionProfile::read_only || prof == ModePermissionProfile::yolo;
}

std::vector<std::string> ModeController::ids() const {
  std::vector<std::string> out;
  out.reserve(modes_.size());
  for (const auto& mode : modes_) {
    out.push_back(mode.id);
  }
  return out;
}

bool ModeController::restore_from_session(const std::string& stored, std::string* notice) {
  if (stored.empty() || stored == "act") {
    current_id_ = "act";
    return true;
  }
  if (find(stored) != nullptr) {
    current_id_ = stored;
    return true;
  }
  current_id_ = "act";
  if (notice != nullptr) {
    *notice = "Unknown mode '" + stored + "'; using Act.";
  }
  return false;
}

} // namespace niminal::app
