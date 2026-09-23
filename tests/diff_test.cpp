#include "diff.hpp"

#include <iostream>

using niminal::app::make_tool_diff;

int main() {
  auto edit =
      make_tool_diff("edit", nlohmann::json{{"old_text", "one\ntwo"}, {"new_text", "one\nchanged"}},
                     false, "OK — edited file.cpp");
  if (!edit.changed || edit.created || edit.body != "- one\n- two\n+ one\n+ changed\n") {
    std::cerr << "edit hunk was not rendered like niminal\n";
    return 1;
  }

  auto create = make_tool_diff("write", nlohmann::json{{"content", "new file\n"}}, true,
                               "OK — wrote new.txt");
  if (!create.changed || !create.created || create.body != "+ 1 | new file\n") {
    std::cerr << "write hunk was not rendered like niminal\n";
    return 1;
  }

  auto numbered = make_tool_diff("edit", nlohmann::json{{"old_text", "old"}, {"new_text", "new"}},
                                 false, "OK — edited file.cpp\nlines: 12-12 > 12-12");
  if (numbered.body != "- 12 | old\n+ 12 | new\n") {
    std::cerr << "edit line numbers were not rendered\n";
    return 1;
  }

  auto replacements = make_tool_diff(
      "edit",
      nlohmann::json{{"replacements", nlohmann::json::array(
                                          {nlohmann::json{{"old_text", "a"}, {"new_text", "A"}},
                                           nlohmann::json{{"old_text", "b"}, {"new_text", "B"}}})}},
      false, "lines: 2-2 > 2-2, 20-20 > 20-20");
  if (replacements.body != "-  2 | a\n+  2 | A\n- 20 | b\n+ 20 | B\n") {
    std::cerr << "multiple edit hunks were not rendered\n";
    return 1;
  }

  if (make_tool_diff("edit", nlohmann::json::object(), false, {}).changed) {
    std::cerr << "empty edit should not have a hunk\n";
    return 1;
  }
  return 0;
}
