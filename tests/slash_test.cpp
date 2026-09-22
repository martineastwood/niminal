#include "slash.hpp"

#include <iostream>

int main() {
  using niminal::app::parse_user_bash;

  auto fail = [](const char* msg) {
    std::cerr << msg << '\n';
    return 1;
  };

  if (parse_user_bash("hello")) {
    return fail("plain text");
  }
  if (parse_user_bash("!")) {
    return fail("bare !");
  }
  if (parse_user_bash("!!")) {
    return fail("bare !!");
  }
  if (parse_user_bash("!   ")) {
    return fail("! with spaces only");
  }

  auto single = parse_user_bash("! git status");
  if (!single || single->exclude_from_context || single->command != "git status") {
    return fail("! git status");
  }

  auto hidden = parse_user_bash("!!pwd");
  if (!hidden || !hidden->exclude_from_context || hidden->command != "pwd") {
    return fail("!!pwd");
  }

  auto spaced = parse_user_bash("!!  echo hi");
  if (!spaced || !spaced->exclude_from_context || spaced->command != "echo hi") {
    return fail("!! echo hi");
  }

  return 0;
}
