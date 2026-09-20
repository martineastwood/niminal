#include "diff.hpp"

#include <iostream>

using niminal::app::make_file_diff;

int main() {
  auto edit = make_file_diff(true, "one\ntwo\n", true, "one\nchanged\n");
  if (!edit.changed || edit.created || edit.body.find("-two") == std::string::npos ||
      edit.body.find("+changed") == std::string::npos) {
    std::cerr << "edit diff was not rendered\n";
    return 1;
  }

  auto create = make_file_diff(false, {}, true, "new file\n");
  if (!create.changed || !create.created ||
      create.body.find("+new file") == std::string::npos) {
    std::cerr << "create diff was not rendered\n";
    return 1;
  }

  if (make_file_diff(true, "same", true, "same").changed) {
    std::cerr << "unchanged file should not have a diff\n";
    return 1;
  }
  return 0;
}
