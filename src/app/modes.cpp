#include "modes.hpp"

#include <niminal/text.hpp>

#include <cassert>
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
  extension_modes_ = std::move(modes);
  rebuild_modes();
}

void ModeController::refresh(niminal::Agent& agent,
                             const std::function<void(niminal::Agent&)>& prepare) {
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
  return lower_copy(trim_copy(std::move(name)));
}

bool ModeController::tool_allowed(const std::vector<std::string>& allowed,
                                  const std::string& name) {
  return allowed.empty() || std::ranges::find(allowed, normalize_tool_name(name)) != allowed.end();
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

void ModeController::cycle_next() {
  const auto it =
      std::ranges::find_if(modes_, [&](const ModeSpec& mode) { return mode.id == current_id_; });
  const size_t index = it == modes_.end() ? 0 : static_cast<size_t>(it - modes_.begin());
  current_id_ = modes_[(index + 1) % modes_.size()].id;
}

std::string ModeController::label() const {
  return require_current().label;
}

std::string ModeController::mode_prompt() const {
  return require_current().prompt;
}

bool ModeController::skips_tool_approval() const {
  const auto prof = require_current().profile;
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
