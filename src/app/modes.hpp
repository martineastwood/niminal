#pragma once

#include <niminal/agent.hpp>

#include <functional>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace niminal::app {

enum class ModePermissionProfile { read_only, ask, yolo };

struct ModeSpec {
  std::string id;
  std::string label;
  std::string prompt;
  std::vector<std::string> tool_allowlist;
  ModePermissionProfile profile = ModePermissionProfile::ask;
};

class ModeController {
public:
  ModeController();

  void set_extension_modes(std::vector<ModeSpec> modes);
  // restore catalog → prepare → capture → apply current mode filter
  void refresh(niminal::Agent& agent, std::function<void(niminal::Agent&)> prepare = {});
  void apply_tools(niminal::Agent& agent) const;

  bool set_mode(std::string_view id);
  bool cycle_next();
  const std::string& id() const { return current_id_; }
  std::string label() const;
  std::string mode_prompt() const;
  ModePermissionProfile profile() const;
  bool skips_tool_approval() const;
  std::vector<std::string> ids() const;

  bool restore_from_session(const std::string& stored, std::string* notice);

  static std::optional<ModePermissionProfile> parse_profile(std::string_view name);
  static std::string normalize_tool_name(std::string name);

private:
  std::vector<ModeSpec> modes_;
  std::vector<ModeSpec> extension_modes_;
  std::string current_id_ = "act";
  std::vector<niminal::Tool> catalog_;

  void rebuild_modes();
  const ModeSpec* find(std::string_view id) const;
  const ModeSpec& require_current() const;
};

} // namespace niminal::app
