#pragma once

#include <filesystem>
#include <string>
#include <vector>

namespace niminal::app {

enum class TrustOverride { default_value, approve, deny };

struct ProjectTrust {
  std::filesystem::path workspace;
  bool required = false;
  bool trusted = true;
  bool prompt = false;
  std::vector<std::filesystem::path> resources;
};

std::filesystem::path canonical_workspace(const std::filesystem::path& workspace);
std::filesystem::path trust_path();
std::vector<std::filesystem::path> project_trust_resources(const std::filesystem::path& workspace);
bool project_resources_trusted(const std::filesystem::path& workspace);
void set_project_resources_trusted(const std::filesystem::path& workspace, bool trusted);
void save_project_trust(const std::filesystem::path& workspace, bool trusted);
ProjectTrust resolve_project_trust(const std::filesystem::path& workspace,
                                   TrustOverride override_value = TrustOverride::default_value);

} // namespace niminal::app
