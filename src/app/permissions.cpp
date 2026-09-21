#include "permissions.hpp"

#include "trust.hpp"

#include <algorithm>
#include <cctype>
#include <fstream>
#include <nlohmann/json.hpp>
#include <sstream>
#include <stdexcept>

namespace niminal::app {
namespace fs = std::filesystem;
using json = nlohmann::json;

namespace {

json call_input(const niminal::ToolCall& call) {
  if (call.arguments.empty()) {
    return json::object();
  }
  try {
    auto input = json::parse(call.arguments);
    return input.is_object() ? input : json::object();
  } catch (...) {
    return json::object();
  }
}

std::string command_from(const niminal::ToolCall& call) {
  auto input = call_input(call);
  return input.value("command", std::string());
}

bool workspace_tool(const std::string& name) {
  return name == "read" || name == "grep" || name == "glob" || name == "ls" || name == "edit" ||
         name == "write" || name == "skill";
}

} // namespace

std::string normalized_command(const std::string& command) {
  std::istringstream input(command);
  std::ostringstream output;
  std::string part;
  bool first = true;
  while (input >> part) {
    if (!first) {
      output << ' ';
    }
    output << part;
    first = false;
  }
  return output.str();
}

std::string permission_key(const niminal::ToolCall& call) {
  if (call.name == "bash") {
    return "bash:" + normalized_command(command_from(call));
  }
  return "tool:" + call.name;
}

std::string permission_description(const niminal::ToolCall& call) {
  if (call.name == "bash") {
    return normalized_command(command_from(call));
  }
  return call.name;
}

bool dangerous_command(const std::string& command) {
  std::string value = " " + command + " ";
  for (char& c : value) {
    c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
  }
  constexpr const char* markers[] = {
      " rm ",          " git reset ", " git clean ", " git checkout -- ",
      " git restore ", " sudo ",      " curl ",      " wget ",
      " ssh ",         " scp ",       " chmod ",     " chown ",
      " kill ",        " pkill ",     " dd ",        " mkfs ",
      " shutdown ",    " reboot ",
  };
  return std::any_of(std::begin(markers), std::end(markers),
                     [&](const char* marker) { return value.find(marker) != std::string::npos; });
}

bool can_remember(const niminal::ToolCall& call) {
  return call.name != "bash" || !dangerous_command(command_from(call));
}

PermissionPolicy::PermissionPolicy(const fs::path& workspace)
    : workspace_(canonical_workspace(workspace)),
      project_path_(workspace_ / ".niminal" / "permissions.json") {
  reload_project();
}

void PermissionPolicy::reload_project() {
  project_allows_.clear();
  if (!project_resources_trusted(workspace_)) {
    return;
  }
  std::error_code ec;
  if (!fs::is_regular_file(project_path_, ec)) {
    return;
  }
  std::ifstream in(project_path_);
  if (!in) {
    return;
  }
  std::ostringstream text;
  text << in.rdbuf();
  try {
    auto doc = json::parse(text.str());
    auto allow = doc.value("allow", json::array());
    if (!allow.is_array()) {
      return;
    }
    for (const auto& item : allow) {
      if (item.is_string()) {
        project_allows_.insert(item.get<std::string>());
      }
    }
  } catch (...) {
  }
}

PermissionCheck PermissionPolicy::check(const niminal::ToolCall& call) const {
  if (call.name.empty()) {
    return PermissionCheck::deny;
  }
  if (workspace_tool(call.name)) {
    return PermissionCheck::allow;
  }
  if (call.name == "bash" && dangerous_command(command_from(call))) {
    return PermissionCheck::ask;
  }
  auto key = permission_key(call);
  if (session_allows_.contains(key)) {
    return PermissionCheck::allow;
  }
  if (project_resources_trusted(workspace_) && project_allows_.contains(key)) {
    return PermissionCheck::allow;
  }
  return PermissionCheck::ask;
}

void PermissionPolicy::persist_project() const {
  fs::create_directories(project_path_.parent_path());
  json doc = json{{"allow", json::array()}};
  for (const auto& key : project_allows_) {
    doc["allow"].push_back(key);
  }
  auto tmp = project_path_;
  tmp += ".tmp";
  {
    std::ofstream out(tmp, std::ios::binary | std::ios::trunc);
    if (!out) {
      throw std::runtime_error("cannot write " + project_path_.string());
    }
    out << doc.dump(2) << '\n';
  }
  fs::rename(tmp, project_path_);
}

void PermissionPolicy::remember(const niminal::ToolCall& call, PermissionDecision decision) {
  if (!can_remember(call)) {
    return;
  }
  auto key = permission_key(call);
  if (decision == PermissionDecision::allow_session) {
    session_allows_.insert(std::move(key));
  } else if (decision == PermissionDecision::allow_project) {
    project_allows_.insert(std::move(key));
    persist_project();
  }
}

void PermissionPolicy::clear_project() {
  project_allows_.clear();
  persist_project();
}

std::string PermissionPolicy::describe() const {
  std::ostringstream out;
  out << "Permission grants:";
  if (session_allows_.empty() && project_allows_.empty()) {
    out << "\n  (none)";
    return out.str();
  }
  for (const auto& key : project_allows_) {
    out << "\n  project  " << key;
  }
  for (const auto& key : session_allows_) {
    out << "\n  session  " << key;
  }
  return out.str();
}

} // namespace niminal::app
