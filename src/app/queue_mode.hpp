#pragma once

#include <string_view>

namespace niminal::app {

inline bool valid_queue_mode(std::string_view mode) {
  return mode == "all" || mode == "one-at-a-time";
}

} // namespace niminal::app
