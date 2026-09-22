#include "theme.hpp"

#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>

using niminal::app::parse_theme_mode;
using niminal::app::resolve_theme;
using niminal::app::theme_from_colorfgbg;
using niminal::app::theme_from_osc_reply;
using niminal::app::theme_mode_name;
using niminal::app::ThemeMode;

int main() {
  if (!parse_theme_mode("auto") || *parse_theme_mode("auto") != ThemeMode::automatic) {
    return 1;
  }
  if (*parse_theme_mode("LIGHT") != ThemeMode::light) {
    return 1;
  }
  if (*parse_theme_mode("dark") != ThemeMode::dark) {
    return 1;
  }
  if (parse_theme_mode("solarized")) {
    return 1;
  }
  if (std::string(theme_mode_name(ThemeMode::automatic)) != "auto") {
    return 1;
  }

  if (*theme_from_osc_reply("\033]11;rgb:0000/0000/0000\033\\") != ThemeMode::dark) {
    return 1;
  }
  if (*theme_from_osc_reply("\033]11;rgb:ffff/ffff/ffff\a") != ThemeMode::light) {
    return 1;
  }
  if (*theme_from_osc_reply("rgb:1e1e/1e1e/1e1e") != ThemeMode::dark) {
    return 1;
  }
  if (*theme_from_osc_reply("rgb:eee/eee/eee") != ThemeMode::light) {
    return 1;
  }
  if (theme_from_osc_reply("\033]11;?\033\\")) {
    return 1;
  }

  if (*theme_from_colorfgbg("15;0") != ThemeMode::dark) {
    return 1;
  }
  if (*theme_from_colorfgbg("0;15") != ThemeMode::light) {
    return 1;
  }
  if (*theme_from_colorfgbg("0;7") != ThemeMode::light) {
    return 1;
  }
  if (*theme_from_colorfgbg("0;8") != ThemeMode::dark) {
    return 1;
  }
  if (theme_from_colorfgbg("15")) {
    return 1;
  }
  if (theme_from_colorfgbg("0;none")) {
    return 1;
  }

  if (resolve_theme(ThemeMode::dark).input_fg == resolve_theme(ThemeMode::light).input_fg) {
    return 1;
  }

  const auto root = std::filesystem::temp_directory_path() / "niminal-theme-test";
  std::filesystem::create_directories(root / ".niminal" / "themes");
  setenv("HOME", root.c_str(), 1);
  {
    std::ofstream out(root / ".niminal" / "themes" / "ocean.json");
    out << R"({"base":"light","colors":{"accent":"#123456","input_bg":"#abcdef"}})";
  }
  const auto custom = niminal::app::load_theme("ocean");
  if (!custom || custom->accent != ftxui::Color::RGB(0x12, 0x34, 0x56) ||
      custom->code != resolve_theme(ThemeMode::light).code ||
      niminal::app::theme_names().back() != "ocean")
    return 1;
  {
    std::ofstream out(root / ".niminal" / "themes" / "broken.json");
    out << R"({"colors":{"accent":"red"}})";
  }
  if (niminal::app::load_theme("broken") || niminal::app::load_theme("missing"))
    return 1;
  std::filesystem::remove_all(root);

  std::cout << "theme ok\n";
  return 0;
}
