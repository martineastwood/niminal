#pragma once

#include <string>
#include <string_view>

namespace niminal::app {

struct FileDiff {
  bool changed = false;
  bool created = false;
  std::string body;
};

FileDiff make_file_diff(bool before_exists, std::string_view before,
                        bool after_exists, std::string_view after);

}  // namespace niminal::app
