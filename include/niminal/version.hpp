#pragma once

#include <string>
#include <string_view>

namespace niminal {

inline constexpr std::string_view kName = "niminal";
inline constexpr std::string_view kVersion = "0.1.0";

// Line printed by --version and shown by /version in the TUI.
inline std::string version_string() {
  return std::string(kName).append(" ").append(kVersion);
}

} // namespace niminal
