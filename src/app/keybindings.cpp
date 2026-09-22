#include "keybindings.hpp"

#include "config.hpp"

#include <nlohmann/json.hpp>

#include <algorithm>
#include <array>
#include <cctype>
#include <fstream>
#include <optional>
#include <string_view>
#include <utility>

namespace niminal::app {
namespace {

enum class Context { normal, approval, both };

struct ActionSpec {
  std::string_view id;
  std::string_view defaults;
  Context context;
};

constexpr std::array kSpecs = {
    ActionSpec{"composer.submit", "enter,ctrl+d,ctrl+s,ctrl+enter", Context::normal},
    ActionSpec{"composer.newline", "alt+j,shift+enter", Context::normal},
    ActionSpec{"composer.wordLeft", "alt+left", Context::normal},
    ActionSpec{"composer.wordRight", "alt+right", Context::normal},
    ActionSpec{"composer.draftStart", "ctrl+left,ctrl+a", Context::normal},
    ActionSpec{"composer.draftEnd", "ctrl+right,ctrl+e", Context::normal},
    ActionSpec{"composer.previous", "up", Context::normal},
    ActionSpec{"composer.next", "down", Context::normal},
    ActionSpec{"composer.complete", "tab", Context::normal},
    ActionSpec{"composer.completePrevious", "shift+tab", Context::normal},
    ActionSpec{"composer.cancel", "escape", Context::normal},
    ActionSpec{"composer.editQueued", "alt+up,shift+left", Context::normal},
    ActionSpec{"composer.paste", "ctrl+v", Context::normal},
    ActionSpec{"composer.externalEditor", "ctrl+g", Context::normal},
    ActionSpec{"transcript.scrollUp", "pageup", Context::normal},
    ActionSpec{"transcript.scrollDown", "pagedown", Context::normal},
    ActionSpec{"transcript.toggleLast", "ctrl+o", Context::normal},
    ActionSpec{"transcript.toggleAll", "ctrl+shift+o", Context::normal},
    ActionSpec{"app.quit", "ctrl+c", Context::both},
    ActionSpec{"approval.allowOnce", "enter,1", Context::approval},
    ActionSpec{"approval.allowSession", "s", Context::approval},
    ActionSpec{"approval.allowProject", "p", Context::approval},
    ActionSpec{"approval.deny", "n,escape", Context::approval},
};

static_assert(kSpecs.size() == static_cast<size_t>(KeyAction::count));

size_t index(KeyAction action) {
  return static_cast<size_t>(action);
}

void add(std::vector<std::string>& inputs, std::string input) {
  if (std::ranges::find(inputs, input) == inputs.end()) {
    inputs.push_back(std::move(input));
  }
}

std::string csi_number(int codepoint, int modifier, char suffix) {
  return "\x1b[" + std::to_string(codepoint) + ";" + std::to_string(modifier) + suffix;
}

std::string pretty(std::string_view name) {
  std::string result;
  size_t start = 0;
  while (start < name.size()) {
    const auto end = name.find('+', start);
    auto part = std::string(name.substr(start, end == std::string_view::npos ? end : end - start));
    if (part == "pageup") {
      part = "Page Up";
    } else if (part == "pagedown") {
      part = "Page Down";
    } else if (!part.empty()) {
      part[0] = static_cast<char>(std::toupper(static_cast<unsigned char>(part[0])));
    }
    if (!result.empty()) {
      result += '+';
    }
    result += part;
    if (end == std::string_view::npos) {
      break;
    }
    start = end + 1;
  }
  return result;
}

std::optional<Keybindings::Key> parse_key(std::string_view name) {
  int modifiers = 0;
  size_t start = 0;
  while (true) {
    const auto end = name.find('+', start);
    if (end == std::string_view::npos) {
      break;
    }
    const auto part = name.substr(start, end - start);
    int bit = 0;
    if (part == "shift") {
      bit = 1;
    } else if (part == "alt") {
      bit = 2;
    } else if (part == "ctrl") {
      bit = 4;
    }
    if (bit == 0 || (modifiers & bit) != 0) {
      return std::nullopt;
    }
    modifiers |= bit;
    start = end + 1;
  }
  const auto base = name.substr(start);
  if (base.empty()) {
    return std::nullopt;
  }

  Keybindings::Key key{std::string(name), {}};
  auto& inputs = key.inputs;
  const int parameter = modifiers + 1;
  if (base.size() == 1 &&
      ((base[0] >= 'a' && base[0] <= 'z') || (base[0] >= '0' && base[0] <= '9'))) {
    const bool letter = base[0] >= 'a' && base[0] <= 'z';
    if (letter && (base == "i" || base == "j" || base == "m") && modifiers == 4) {
      return std::nullopt; // These bytes are indistinguishable from Tab or Enter.
    }
    constexpr std::string_view shifted_digits = ")!@#$%^&*(";
    char shifted = base[0];
    if ((modifiers & 1) != 0) {
      shifted = letter ? static_cast<char>(std::toupper(static_cast<unsigned char>(base[0])))
                       : shifted_digits[static_cast<size_t>(base[0] - '0')];
    }
    if (modifiers == 0 || modifiers == 1) {
      add(inputs, std::string(1, shifted));
    }
    if (letter && modifiers == 4) {
      add(inputs, std::string(1, static_cast<char>(base[0] - 'a' + 1)));
    }
    if ((modifiers & 2) != 0 && (modifiers & 4) == 0) {
      add(inputs, "\x1b" + std::string(1, shifted));
    }
    if (modifiers != 0) {
      add(inputs, csi_number(static_cast<unsigned char>(shifted), parameter, 'u'));
      add(inputs, csi_number(static_cast<unsigned char>(shifted), parameter, '~'));
      add(inputs, "\x1b[27;" + std::to_string(parameter) + ";" +
                      std::to_string(static_cast<unsigned char>(shifted)) + "~");
      if (shifted != base[0]) {
        add(inputs, csi_number(static_cast<unsigned char>(base[0]), parameter, 'u'));
        add(inputs, csi_number(static_cast<unsigned char>(base[0]), parameter, '~'));
      }
    }
  } else {
    struct NamedKey {
      std::string_view name;
      std::string_view input;
      int codepoint;
      char csi_suffix;
    };
    constexpr std::array named = {
        NamedKey{"enter", "\n", 13, 0},        NamedKey{"escape", "\x1b", 27, 0},
        NamedKey{"tab", "\t", 9, 0},           NamedKey{"up", "\x1b[A", 65, 'A'},
        NamedKey{"down", "\x1b[B", 66, 'B'},   NamedKey{"right", "\x1b[C", 67, 'C'},
        NamedKey{"left", "\x1b[D", 68, 'D'},   NamedKey{"home", "\x1b[H", 72, 'H'},
        NamedKey{"end", "\x1b[F", 70, 'F'},    NamedKey{"pageup", "\x1b[5~", 5, 0},
        NamedKey{"pagedown", "\x1b[6~", 6, 0}, NamedKey{"backspace", "\x7f", 127, 0},
        NamedKey{"delete", "\x1b[3~", 3, 0},
    };
    const auto item =
        std::ranges::find_if(named, [&](const NamedKey& entry) { return entry.name == base; });
    if (item == named.end()) {
      return std::nullopt;
    }
    if (modifiers == 0) {
      add(inputs, std::string(item->input));
    } else if (base == "tab" && modifiers == 1) {
      add(inputs, "\x1b[Z");
    } else if (item->csi_suffix != 0) {
      add(inputs, "\x1b[1;" + std::to_string(parameter) + item->csi_suffix);
      add(inputs,
          "\x1b[" + std::string(1, item->csi_suffix) + ";" + std::to_string(parameter) + "u");
      add(inputs,
          "\x1b[27;" + std::to_string(parameter) + ";" + std::to_string(item->codepoint) + "~");
    } else {
      add(inputs, csi_number(item->codepoint, parameter, 'u'));
      add(inputs, csi_number(item->codepoint, parameter, '~'));
      add(inputs,
          "\x1b[27;" + std::to_string(parameter) + ";" + std::to_string(item->codepoint) + "~");
      if (base == "enter" && modifiers == 2) {
        add(inputs, "\x1b\r");
        add(inputs, "\x1b\n");
      }
    }
  }

  if (name == "alt+j") {
    add(inputs, "\x1bJ");
    add(inputs, "\x1b[74;3u");
    add(inputs, "∆");
  } else if (name == "alt+left") {
    add(inputs, "\x1b[1;3D");
    add(inputs, "\x1b"
                "b");
  } else if (name == "alt+right") {
    add(inputs, "\x1b[1;3C");
    add(inputs, "\x1b"
                "f");
  } else if (name == "alt+up") {
    add(inputs, "\x1b\x1b[A");
  } else if (name == "shift+enter") {
    add(inputs, "\x1b\r");
    add(inputs, "\x1b\n");
  } else if (name == "ctrl+v") {
    add(inputs, "\x1b[118;2u");
    add(inputs, "\x1b[118;8u");
    add(inputs, "\x1b[118;9u");
  }
  return key;
}

bool overlapping(Context a, Context b) {
  return a == Context::both || b == Context::both || a == b;
}

} // namespace

Keybindings::Keybindings() {
  for (size_t i = 0; i < kSpecs.size(); ++i) {
    auto names = kSpecs[i].defaults;
    while (!names.empty()) {
      const auto comma = names.find(',');
      const auto name = names.substr(0, comma);
      bindings_[i].push_back(*parse_key(name));
      if (comma == std::string_view::npos) {
        break;
      }
      names.remove_prefix(comma + 1);
    }
  }
}

bool Keybindings::matches(KeyAction action, const ftxui::Event& event) const {
  const auto& bindings = bindings_[index(action)];
  return std::ranges::any_of(bindings, [&](const Key& key) {
    return std::ranges::find(key.inputs, event.input()) != key.inputs.end();
  });
}

std::string Keybindings::label(KeyAction action) const {
  std::string out;
  for (const auto& key : bindings_[index(action)]) {
    if (!out.empty()) {
      out += " / ";
    }
    out += pretty(key.name);
  }
  return out;
}

std::filesystem::path keybindings_path() {
  return config_path().parent_path() / "keybindings.json";
}

KeybindingsLoad load_keybindings_file(const std::filesystem::path& path) {
  KeybindingsLoad result;
  std::error_code ec;
  if (!std::filesystem::exists(path, ec)) {
    if (ec) {
      result.error = "cannot read " + path.string() + ": " + ec.message();
    }
    return result;
  }
  std::ifstream in(path);
  if (!in) {
    result.error = "cannot read " + path.string();
    return result;
  }
  try {
    const auto doc = nlohmann::json::parse(in);
    if (!doc.is_object()) {
      result.error = path.string() + ": expected a JSON object";
      return result;
    }
    Keybindings custom;
    for (const auto& [id, value] : doc.items()) {
      const auto spec = std::ranges::find_if(
          kSpecs, [&](const ActionSpec& candidate) { return candidate.id == id; });
      if (spec == kSpecs.end()) {
        result.error = path.string() + ": unknown action '" + id + "'";
        return result;
      }
      std::vector<std::string> names;
      if (value.is_string()) {
        names.push_back(value.get<std::string>());
      } else if (value.is_array()) {
        for (const auto& entry : value) {
          if (!entry.is_string()) {
            result.error = path.string() + ": '" + id + "' needs key strings";
            return result;
          }
          names.push_back(entry.get<std::string>());
        }
      }
      if (names.empty()) {
        result.error = path.string() + ": '" + id + "' needs at least one key";
        return result;
      }
      auto& target = custom.bindings_[static_cast<size_t>(spec - kSpecs.begin())];
      target.clear();
      for (const auto& name : names) {
        auto parsed = parse_key(name);
        if (!parsed) {
          result.error = path.string() + ": unsupported key '" + name + "' for '" + id + "'";
          return result;
        }
        target.push_back(std::move(*parsed));
      }
    }
    for (size_t i = 0; i < kSpecs.size(); ++i) {
      for (size_t j = i + 1; j < kSpecs.size(); ++j) {
        if (!overlapping(kSpecs[i].context, kSpecs[j].context)) {
          continue;
        }
        for (const auto& left : custom.bindings_[i]) {
          for (const auto& right : custom.bindings_[j]) {
            for (const auto& input : left.inputs) {
              if (std::ranges::find(right.inputs, input) != right.inputs.end()) {
                result.error = path.string() + ": '" + left.name + "' conflicts between '" +
                               std::string(kSpecs[i].id) + "' and '" + std::string(kSpecs[j].id) +
                               "'";
                return result;
              }
            }
          }
        }
      }
    }
    result.bindings = std::move(custom);
  } catch (const nlohmann::json::exception& error) {
    result.error = path.string() + ": " + error.what();
  }
  return result;
}

KeybindingsLoad load_keybindings() {
  try {
    return load_keybindings_file(keybindings_path());
  } catch (const std::exception& error) {
    KeybindingsLoad result;
    result.error = error.what();
    return result;
  }
}

std::string busy_wait_message(const Keybindings& keybindings) {
  return "wait for the turn to finish, or " + keybindings.label(KeyAction::cancel) +
         " to interrupt";
}

} // namespace niminal::app
