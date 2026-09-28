#pragma once

#include <niminal/json.hpp>

#include <string>

namespace niminal::detail {

inline niminal::json parse_tool_input(const std::string& arguments) {
  if (arguments.empty())
    return json_object();
  try {
    auto input = json_parse(arguments);
    return input.is_object() ? input : json_object();
  } catch (...) {
    return json_object();
  }
}

} // namespace niminal::detail
