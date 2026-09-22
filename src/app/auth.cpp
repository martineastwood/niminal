#include "auth.hpp"

#include <nlohmann/json.hpp>

#include <cstdlib>
#include <fstream>
#include <sstream>
#include <stdexcept>

namespace niminal::app {
namespace fs = std::filesystem;
using json = nlohmann::json;

namespace {

std::string resolve_key(std::string value) {
  if (value.size() < 2 || value.front() != '$') {
    return value;
  }

  std::string name;
  if (value.size() >= 4 && value[1] == '{' && value.back() == '}') {
    name = value.substr(2, value.size() - 3);
  } else {
    name = value.substr(1);
  }
  if (name.empty()) {
    return {};
  }
  const char* resolved = std::getenv(name.c_str());
  return resolved == nullptr ? std::string{} : std::string(resolved);
}

std::string configured_key(const json& entry) {
  if (entry.is_string()) {
    return entry.get<std::string>();
  }
  if (entry.is_object() && entry.contains("key") && entry["key"].is_string()) {
    return entry["key"].get<std::string>();
  }
  return {};
}

} // namespace

fs::path auth_path() {
  const char* home = std::getenv("HOME");
  if ((home == nullptr) || ((*home) == 0)) {
    throw std::runtime_error("HOME is not set; cannot load ~/.niminal/auth.json");
  }
  return fs::path(home) / ".niminal" / "auth.json";
}

std::string read_auth_key(std::string_view provider, const fs::path& path) {
  std::error_code ec;
  if (!fs::is_regular_file(path, ec)) {
    return {};
  }
  std::ifstream in(path);
  if (!in) {
    return {};
  }

  std::ostringstream text;
  text << in.rdbuf();
  try {
    const auto doc = json::parse(text.str());
    if (!doc.is_object()) {
      return {};
    }
    const auto it = doc.find(std::string(provider));
    if (it == doc.end()) {
      return {};
    }
    return resolve_key(configured_key(*it));
  } catch (...) {
    return {};
  }
}

std::string read_auth_key(std::string_view provider) {
  try {
    return read_auth_key(provider, auth_path());
  } catch (...) {
    return {};
  }
}

} // namespace niminal::app
