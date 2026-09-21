#pragma once

#include <nlohmann/json.hpp>

#include <string>
#include <string_view>
#include <vector>

namespace niminal::app {

constexpr const char* kThinkingLevels[] = {"none", "minimal", "low", "medium",
                                           "high", "xhigh",   "max"};

std::string normalize_thinking(std::string_view value);
std::string snap_to_efforts(std::string_view want, const std::vector<std::string>& efforts);
std::vector<std::string> thinking_choices(std::string_view provider, std::string_view model);
std::string thinking_status(std::string_view provider, std::string_view model,
                            std::string_view want);
nlohmann::json thinking_body(std::string_view provider, std::string_view model,
                             std::string_view want);

} // namespace niminal::app
