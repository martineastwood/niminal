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

  using niminal::app::is_extension_slash;
  using niminal::app::skill_slash_error;
  if (skill_slash_error("/tmp", "/help")) {
    return fail("non-skill slash");
  }
  if (!skill_slash_error("/tmp", "/skill:missing")) {
    return fail("unknown skill");
  }
  if (is_extension_slash(nullptr, "/custom")) {
    return fail("null extensions");
  }

  return 0;
}
