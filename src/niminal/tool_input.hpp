#pragma once

#include <nlohmann/json.hpp>

#include <string>

namespace niminal::detail {

inline nlohmann::json parse_tool_input(const std::string& arguments) {
  if (arguments.empty()) {
    return nlohmann::json::object();
  }
  try {
    auto input = nlohmann::json::parse(arguments);
    return input.is_object() ? input : nlohmann::json::object();
  } catch (...) {
    return nlohmann::json::object();
  }
}

} // namespace niminal::detail
