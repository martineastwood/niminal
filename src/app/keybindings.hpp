#pragma once

#include <ftxui/component/event.hpp>

#include <array>
#include <filesystem>
#include <string>
#include <vector>

namespace niminal::app {

enum class KeyAction {
  submit,
  newline,
  word_left,
  word_right,
  draft_start,
  draft_end,
  previous,
  next,
  complete,
  complete_previous,
  cancel,
  edit_queued,
  paste,
  external_editor,
  scroll_up,
  scroll_down,
  toggle_last,
  toggle_all,
  quit,
  allow_once,
  allow_session,
  allow_project,
  deny,
  count,
};

struct KeybindingsLoad;

class Keybindings {
public:
  struct Key {
    std::string name;
    std::vector<std::string> inputs;
  };

  Keybindings();

  bool matches(KeyAction action, const ftxui::Event& event) const;
  std::string label(KeyAction action) const;

private:
  std::array<std::vector<Key>, static_cast<size_t>(KeyAction::count)> bindings_;

  friend struct KeybindingsLoad;
  friend KeybindingsLoad load_keybindings_file(const std::filesystem::path& path);
};

struct KeybindingsLoad {
  Keybindings bindings;
  std::string error;
};

std::filesystem::path keybindings_path();
KeybindingsLoad load_keybindings_file(const std::filesystem::path& path);
KeybindingsLoad load_keybindings();
std::string busy_wait_message(const Keybindings& keybindings);

} // namespace niminal::app
