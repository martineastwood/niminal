#include "modes.hpp"

#include "session.hpp"

#include <iostream>

namespace {

int fail(const char* message) {
  std::cerr << message << '\n';
  return 1;
}

} // namespace

int main() {
  niminal::app::ModeController modes;
  if (modes.id() != "act" || modes.label() != "Act") {
    return fail("default mode is act");
  }
  if (!modes.cycle_next() || modes.id() != "plan") {
    return fail("cycle act to plan");
  }
  niminal::Agent agent;
  agent.tools.push_back(niminal::Tool{"read", "r", niminal::json_object(), {}, true});
  agent.tools.push_back(niminal::Tool{"bash", "b", niminal::json_object(), {}, false});
  agent.tools.push_back(niminal::Tool{"git", "g", niminal::json_object(), {}, true});
  agent.tools.push_back(niminal::Tool{"ask_user", "q", niminal::json_object(), {}, false});
  modes.refresh(agent);
  if (agent.tools.size() != 3) {
    return fail("plan should allow read, git, and ask_user only");
  }
  if (modes.mode_prompt().find("ask_user") == std::string::npos) {
    return fail("plan prompt should tell the model to interview the user");
  }
  if (modes.set_mode("act")) {
    modes.apply_tools(agent);
  }
  if (agent.tools.size() != 4) {
    return fail("act should allow all catalog tools");
  }
  niminal::app::Session session;
  session.add_mode("plan");
  if (session.last_mode() != "plan") {
    return fail("last_mode");
  }
  std::string notice;
  modes.restore_from_session("missing", &notice);
  if (modes.id() != "act" || notice.empty()) {
    return fail("unknown mode fallback");
  }
  return 0;
}
