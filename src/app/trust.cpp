#include "trust.hpp"

#include <nlohmann/json.hpp>

#include <algorithm>
#include <cstdlib>
#include <fstream>
#include <map>
#include <sstream>
#include <stdexcept>
#include <system_error>

namespace niminal::app {
namespace fs = std::filesystem;
using json = nlohmann::json;

namespace {

std::map<std::string, bool> trust_scopes;

void add_file(std::vector<fs::path>& result, const fs::path& root, const std::string& relative) {
  std::error_code ec;
  auto path = root / relative;
  if (fs::is_regular_file(path, ec)) {
    result.push_back(path);
  }
}

void add_skill_manifests(std::vector<fs::path>& result, const fs::path& root,
                         const std::string& relative) {
  std::error_code ec;
  auto base = root / relative;
  if (!fs::is_directory(base, ec)) {
    return;
  }
  std::vector<fs::path> paths;
  for (const auto& entry : fs::directory_iterator(base, ec)) {
    auto manifest = entry.path() / "SKILL.md";
    if (entry.is_directory(ec) && fs::is_regular_file(manifest, ec)) {
      paths.push_back(manifest);
    }
  }
  std::sort(paths.begin(), paths.end());
  result.insert(result.end(), paths.begin(), paths.end());
}

void add_markdown_files(std::vector<fs::path>& result, const fs::path& root,
                        const std::string& relative) {
  std::error_code ec;
  auto base = root / relative;
  if (!fs::is_directory(base, ec)) {
    return;
  }
  std::vector<fs::path> paths;
  for (const auto& entry : fs::directory_iterator(base, ec)) {
    if (entry.is_regular_file(ec) && entry.path().extension() == ".md") {
      paths.push_back(entry.path());
    }
  }
  std::sort(paths.begin(), paths.end());
  result.insert(result.end(), paths.begin(), paths.end());
}

void add_manifests(std::vector<fs::path>& result, const fs::path& root, const std::string& relative,
                   const char* name) {
  std::error_code ec;
  auto base = root / relative;
  if (!fs::is_directory(base, ec)) {
    return;
  }
  std::vector<fs::path> paths;
  for (const auto& entry : fs::directory_iterator(base, ec)) {
    auto manifest = entry.path() / name;
    if (entry.is_directory(ec) && fs::is_regular_file(manifest, ec)) {
      paths.push_back(manifest);
    }
  }
  std::sort(paths.begin(), paths.end());
  result.insert(result.end(), paths.begin(), paths.end());
}

json read_trust_doc() {
  std::error_code ec;
  fs::path path;
  try {
    path = trust_path();
  } catch (...) {
    return json::object();
  }
  if (!fs::is_regular_file(path, ec)) {
    return json::object();
  }
  std::ifstream in(path);
  if (!in) {
    return json::object();
  }
  std::ostringstream text;
  text << in.rdbuf();
  try {
    auto doc = json::parse(text.str());
    return doc.is_object() ? doc : json::object();
  } catch (...) {
    return json::object();
  }
}

std::pair<bool, bool> saved_trust(const fs::path& workspace) {
  auto doc = read_trust_doc();
  auto projects = doc.value("projects", json::object());
  if (!projects.is_object()) {
    return {false, false};
  }
  auto current = canonical_workspace(workspace);
  while (true) {
    auto it = projects.find(current.string());
    if (it != projects.end() && it->is_boolean()) {
      return {true, it->get<bool>()};
    }
    auto parent = current.parent_path();
    if (parent == current) {
      break;
    }
    current = std::move(parent);
  }
  return {false, false};
}

} // namespace

fs::path canonical_workspace(const fs::path& workspace) {
  std::error_code ec;
  auto path = fs::weakly_canonical(fs::absolute(workspace), ec);
  return ec ? fs::absolute(workspace).lexically_normal() : path;
}

fs::path trust_path() {
  const char* home = std::getenv("HOME");
  if ((home == nullptr) || ((*home) == 0)) {
    throw std::runtime_error("HOME is not set; cannot use ~/.niminal/trust.json");
  }
  return fs::path(home) / ".niminal" / "trust.json";
}

std::vector<fs::path> project_trust_resources(const fs::path& workspace) {
  const auto root = canonical_workspace(workspace);
  std::vector<fs::path> result;
  add_file(result, root, ".niminal/permissions.json");
  add_file(result, root, ".niminal/SYSTEM.md");
  add_file(result, root, ".niminal/APPEND_SYSTEM.md");
  for (const auto& path : {std::string(".agent/skills"), std::string(".agents/skills"),
                           std::string(".niminal/skills")}) {
    add_skill_manifests(result, root, path);
  }
  for (const auto& path : {std::string(".agent/prompts"), std::string(".agents/prompts"),
                           std::string(".niminal/prompts")}) {
    add_markdown_files(result, root, path);
  }
  for (const auto& path :
       {std::string(".agent/tools"), std::string(".agents/tools"), std::string(".niminal/tools")}) {
    add_manifests(result, root, path, "tool.json");
  }
  add_manifests(result, root, ".agents/extensions", "extension.json");
  add_manifests(result, root, ".niminal/extensions", "extension.json");
  return result;
}

bool project_resources_trusted(const fs::path& workspace) {
  const auto key = canonical_workspace(workspace).string();
  auto it = trust_scopes.find(key);
  return it == trust_scopes.end() ? true : it->second;
}

void set_project_resources_trusted(const fs::path& workspace, bool trusted) {
  trust_scopes[canonical_workspace(workspace).string()] = trusted;
}

void save_project_trust(const fs::path& workspace, bool trusted) {
  auto path = trust_path();
  fs::create_directories(path.parent_path());
  auto doc = read_trust_doc();
  if (!doc.contains("projects") || !doc["projects"].is_object()) {
    doc["projects"] = json::object();
  }
  doc["projects"][canonical_workspace(workspace).string()] = trusted;
  auto tmp = path;
  tmp += ".tmp";
  {
    std::ofstream out(tmp, std::ios::binary | std::ios::trunc);
    if (!out) {
      throw std::runtime_error("cannot write " + path.string());
    }
    out << doc.dump(2) << '\n';
  }
  fs::rename(tmp, path);
}

ProjectTrust resolve_project_trust(const fs::path& workspace, TrustOverride override_value) {
  ProjectTrust result;
  result.workspace = canonical_workspace(workspace);
  result.resources = project_trust_resources(result.workspace);
  result.required = !result.resources.empty();
  if (!result.required) {
    return result;
  }
  if (override_value == TrustOverride::approve) {
    result.trusted = true;
  } else if (override_value == TrustOverride::deny) {
    result.trusted = false;
  } else {
    auto [found, trusted] = saved_trust(result.workspace);
    result.prompt = !found;
    result.trusted = found && trusted;
  }
  return result;
}

} // namespace niminal::app
