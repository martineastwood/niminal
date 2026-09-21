#pragma once

#include <niminal/types.hpp>

#include <filesystem>
#include <set>
#include <string>

namespace niminal::app {

enum class PermissionDecision {
  allow_once,
  allow_session,
  allow_project,
  deny,
};

enum class PermissionCheck { allow, ask, deny };

std::string permission_description(const niminal::ToolCall& call);
bool can_remember(const niminal::ToolCall& call);

class PermissionPolicy {
public:
  explicit PermissionPolicy(const std::filesystem::path& workspace);

  PermissionCheck check(const niminal::ToolCall& call) const;
  void remember(const niminal::ToolCall& call, PermissionDecision decision);
  void clear_project();
  void reload_project();
  std::string describe() const;

  const std::filesystem::path& workspace() const { return workspace_; }
  const std::filesystem::path& project_path() const { return project_path_; }

private:
  std::filesystem::path workspace_;
  std::filesystem::path project_path_;
  std::set<std::string> session_allows_;
  std::set<std::string> project_allows_;

  void persist_project() const;
};

} // namespace niminal::app
