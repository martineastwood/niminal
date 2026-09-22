#include "keybindings.hpp"

#include <ftxui/component/event.hpp>

#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>

namespace fs = std::filesystem;
using ftxui::Event;
using niminal::app::KeyAction;
using niminal::app::load_keybindings_file;

namespace {

int fail(const std::string& message) {
  std::cerr << message << '\n';
  return 1;
}

void write(const fs::path& path, const std::string& content) {
  std::ofstream out(path);
  out << content;
}

} // namespace

int main() {
  const auto dir = fs::temp_directory_path() / "niminal-keybindings-test";
  fs::remove_all(dir);
  fs::create_directories(dir);
  const auto path = dir / "keybindings.json";

  auto missing = load_keybindings_file(path);
  if (!missing.error.empty() || !missing.bindings.matches(KeyAction::draft_start, Event::CtrlA) ||
      !missing.bindings.matches(KeyAction::draft_start, Event::ArrowLeftCtrl) ||
      !missing.bindings.matches(KeyAction::newline, Event::Special("\x1b[13;2u")) ||
      !missing.bindings.matches(KeyAction::newline, Event::Character("∆")) ||
      !missing.bindings.matches(KeyAction::edit_queued, Event::Special("\x1b[D;2u")) ||
      !missing.bindings.matches(KeyAction::toggle_all, Event::Special("\x1b[79;6~")) ||
      !missing.bindings.matches(KeyAction::allow_once, Event::Return)) {
    return fail("missing file should use all default terminal variants");
  }

  write(path, "{}");
  if (!load_keybindings_file(path).error.empty()) {
    return fail("default keys in separate UI contexts should coexist");
  }

  write(path, R"({"composer.draftStart":"shift+1"})");
  auto shifted = load_keybindings_file(path);
  if (!shifted.error.empty() ||
      !shifted.bindings.matches(KeyAction::draft_start, Event::Character('!')) ||
      shifted.bindings.matches(KeyAction::draft_start, Event::Character('1'))) {
    return fail("shifted digits should match their typed symbols");
  }

  write(
      path,
      R"({"composer.draftStart":"ctrl+b","composer.draftEnd":["ctrl+f","home"],"approval.deny":"q"})");
  auto custom = load_keybindings_file(path);
  if (!custom.error.empty() || !custom.bindings.matches(KeyAction::draft_start, Event::CtrlB) ||
      custom.bindings.matches(KeyAction::draft_start, Event::CtrlA) ||
      custom.bindings.matches(KeyAction::draft_start, Event::ArrowLeftCtrl) ||
      !custom.bindings.matches(KeyAction::draft_end, Event::CtrlF) ||
      !custom.bindings.matches(KeyAction::draft_end, Event::Home) ||
      !custom.bindings.matches(KeyAction::deny, Event::Character('q')) ||
      custom.bindings.matches(KeyAction::deny, Event::Escape) ||
      !custom.bindings.matches(KeyAction::cancel, Event::Escape) ||
      !custom.bindings.matches(KeyAction::submit, Event::Return) ||
      !custom.bindings.matches(KeyAction::allow_once, Event::Return) ||
      custom.bindings.label(KeyAction::draft_start) != "Ctrl+B") {
    return fail("custom keys should replace their action defaults only");
  }

  write(path, R"({"composer.draftStart":"ctrl+e"})");
  auto conflict = load_keybindings_file(path);
  if (conflict.error.find("conflicts") == std::string::npos ||
      !conflict.bindings.matches(KeyAction::draft_start, Event::CtrlA)) {
    return fail("colliding keys should reject the whole file");
  }

  write(path, R"({"approval.deny":"ctrl+c"})");
  auto global_conflict = load_keybindings_file(path);
  if (global_conflict.error.find("conflicts") == std::string::npos) {
    return fail("quit should conflict with keys in either UI context");
  }

  write(path, R"({"composer.draftStart":"ctrl+b","unknown.action":"f1"})");
  auto unknown = load_keybindings_file(path);
  if (unknown.error.find("unknown action") == std::string::npos ||
      unknown.bindings.matches(KeyAction::draft_start, Event::CtrlB)) {
    return fail("unknown action should reject every override");
  }

  write(path, R"({"composer.draftStart":[]})");
  if (load_keybindings_file(path).error.find("at least one") == std::string::npos) {
    return fail("empty key list should be rejected");
  }

  write(path, R"({"composer.draftStart":"super+left"})");
  if (load_keybindings_file(path).error.find("unsupported key") == std::string::npos) {
    return fail("unsupported modifier should be rejected");
  }

  write(path, "{not json");
  if (load_keybindings_file(path).error.empty()) {
    return fail("malformed JSON should produce a visible error");
  }

  fs::remove_all(dir);
  return 0;
}
