#pragma once

#include <nlohmann/json.hpp>

#include <string>
#include <string_view>

namespace niminal::app {

struct FileDiff {
  bool changed = false;
  bool created = false;
  std::string body;
};

FileDiff make_tool_diff(std::string_view tool_name, const nlohmann::json& input, bool created,
                        std::string_view output);

} // namespace niminal::app
