#include "theme.hpp"

#include <fcntl.h>
#include <termios.h>
#include <unistd.h>

#include <niminal/text.hpp>
#include <nlohmann/json.hpp>

#include <algorithm>
#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <string>
#include <utility>

namespace niminal::app {
namespace {

using namespace ftxui;

bool is_hex(char c) {
  return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F');
}

// Scales a 1-4 digit hex channel to 0-255.
std::optional<int> hex_channel(std::string_view text) {
  if (text.empty() || text.size() > 4) {
    return std::nullopt;
  }
  int value = 0;
  for (char c : text) {
    value <<= 4;
    if (c >= '0' && c <= '9') {
      value += c - '0';
    } else if (c >= 'a' && c <= 'f') {
      value += c - 'a' + 10;
    } else {
      value += c - 'A' + 10;
    }
  }
  return value * 255 / ((1 << (4 * static_cast<int>(text.size()))) - 1);
}

ThemeMode theme_from_luminance(int r, int g, int b) {
  return (2126 * r + 7152 * g + 722 * b) / 10000 < 128 ? ThemeMode::dark : ThemeMode::light;
}

std::optional<std::string> query_terminal_background() {
  const int fd = ::open("/dev/tty", O_RDWR | O_NOCTTY);
  if (fd < 0) {
    return std::nullopt;
  }
  termios saved{};
  if (::tcgetattr(fd, &saved) != 0) {
    ::close(fd);
    return std::nullopt;
  }
  termios raw = saved;
  raw.c_lflag &= ~static_cast<tcflag_t>(ICANON | ECHO);
  raw.c_cc[VMIN] = 0;
  raw.c_cc[VTIME] = 1;
  if (::tcsetattr(fd, TCSANOW, &raw) != 0) {
    ::close(fd);
    return std::nullopt;
  }
  constexpr char kQuery[] = "\033]11;?\033\\";
  std::string reply;
  if (::write(fd, kQuery, sizeof(kQuery) - 1) > 0) {
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(150);
    while (std::chrono::steady_clock::now() < deadline) {
      char buffer[64];
      const ssize_t n = ::read(fd, buffer, sizeof(buffer));
      if (n > 0) {
        reply.append(buffer, static_cast<size_t>(n));
      }
      if (reply.find('\a') != std::string::npos || reply.find("\033\\") != std::string::npos) {
        break;
      }
    }
  }
  ::tcsetattr(fd, TCSANOW, &saved);
  ::close(fd);
  if (reply.empty()) {
    return std::nullopt;
  }
  return reply;
}

const char* const kModeNames[] = {"auto", "light", "dark"};

} // namespace

std::optional<ThemeMode> parse_theme_mode(std::string_view value) {
  const auto normalized = niminal::lower_copy(std::string(value));
  for (int i = 0; i < 3; ++i) {
    if (normalized == kModeNames[i]) {
      return static_cast<ThemeMode>(i);
    }
  }
  return std::nullopt;
}

const char* theme_mode_name(ThemeMode mode) {
  return kModeNames[static_cast<int>(mode)];
}

std::optional<ThemeMode> theme_from_osc_reply(std::string_view reply) {
  const auto at = reply.find("rgb:");
  if (at == std::string_view::npos) {
    return std::nullopt;
  }
  std::string_view body = reply.substr(at + 4);
  int channels[3] = {0, 0, 0};
  for (int i = 0; i < 3; ++i) {
    size_t digits = 0;
    while (digits < body.size() && digits < 4 && is_hex(body[digits])) {
      ++digits;
    }
    auto channel = hex_channel(body.substr(0, digits));
    if (!channel) {
      return std::nullopt;
    }
    channels[i] = *channel;
    body.remove_prefix(digits);
    if (i < 2) {
      if (body.empty() || body.front() != '/') {
        return std::nullopt;
      }
      body.remove_prefix(1);
    }
  }
  return theme_from_luminance(channels[0], channels[1], channels[2]);
}

std::optional<ThemeMode> theme_from_colorfgbg(std::string_view value) {
  const auto sep = value.find_last_of(';');
  if (sep == std::string_view::npos) {
    return std::nullopt;
  }
  auto field = value.substr(sep + 1);
  if (field.empty() || field.size() > 2) {
    return std::nullopt;
  }
  int index = 0;
  for (char c : field) {
    if (c < '0' || c > '9') {
      return std::nullopt;
    }
    index = index * 10 + (c - '0');
  }
  if (index > 15) {
    return std::nullopt;
  }
  return index == 7 || index > 8 ? ThemeMode::light : ThemeMode::dark;
}

ThemeMode detect_terminal_theme() {
  static const ThemeMode detected = [] {
    if (auto reply = query_terminal_background()) {
      if (auto mode = theme_from_osc_reply(*reply)) {
        return *mode;
      }
    }
    if (const char* env = std::getenv("COLORFGBG")) {
      if (auto mode = theme_from_colorfgbg(env)) {
        return *mode;
      }
    }
    return ThemeMode::dark;
  }();
  return detected;
}

Theme resolve_theme(ThemeMode mode) {
  const bool dark = mode == ThemeMode::dark ||
                    (mode == ThemeMode::automatic && detect_terminal_theme() == ThemeMode::dark);
  if (dark) {
    return Theme{
        .accent = Color::CyanLight,
        .code = Color::GreenLight,
        .add = Color::GreenLight,
        .del = Color::RedLight,
        .meta = Color::YellowLight,
        .thinking = Color::MediumPurple1,
        .error = Color::Red,
        .muted = Color::GrayLight,
        .emphasis = Color::YellowLight,
        .italic = Color::MagentaLight,
        .quote = Color::BlueLight,
        .input_fg = Color::White,
        .input_bg = Color::RGB(45, 45, 45),
        .hover_bg = Color::GrayDark,
    };
  }
  // Light terminals: the normal (darker) ANSI colors stay readable on white.
  return Theme{
      .accent = Color::Blue,
      .code = Color::Green,
      .add = Color::Green,
      .del = Color::Red,
      .meta = Color::Magenta,
      .thinking = Color::Purple4,
      .error = Color::Red,
      .muted = Color::GrayDark,
      .emphasis = Color::Magenta,
      .italic = Color::Magenta,
      .quote = Color::Blue,
      .input_fg = Color::Black,
      .input_bg = Color::RGB(234, 234, 234),
      .hover_bg = Color::GrayLight,
  };
}

std::vector<std::string> theme_names() {
  std::vector<std::string> names{"auto", "light", "dark"};
  const char* home = std::getenv("HOME");
  if (!home || !*home) {
    return names;
  }
  std::error_code ec;
  const auto directory = std::filesystem::path(home) / ".niminal" / "themes";
  for (std::filesystem::directory_iterator it(directory, ec), end; !ec && it != end;
       it.increment(ec)) {
    if (it->is_regular_file(ec) && it->path().extension() == ".json") {
      const auto name = it->path().stem().string();
      if (!parse_theme_mode(name)) {
        names.push_back(name);
      }
    }
  }
  std::sort(names.begin() + 3, names.end());
  return names;
}

std::expected<Theme, std::string> load_theme(std::string_view name) {
  if (auto mode = parse_theme_mode(name)) {
    return resolve_theme(*mode);
  }
  const char* home = std::getenv("HOME");
  if (!home || !*home) {
    return std::unexpected("HOME is not set");
  }
  const auto names = theme_names();
  if (std::find(names.begin(), names.end(), name) == names.end()) {
    return std::unexpected("theme not found: " + std::string(name));
  }
  const auto path =
      std::filesystem::path(home) / ".niminal" / "themes" / (std::string(name) + ".json");
  try {
    std::ifstream input(path);
    if (!input) {
      return std::unexpected("cannot read " + path.string());
    }
    const auto doc = nlohmann::json::parse(input);
    if (!doc.is_object()) {
      return std::unexpected("theme must be a JSON object");
    }
    const auto base = doc.value("base", std::string("dark"));
    const auto mode = parse_theme_mode(base);
    if (!mode || *mode == ThemeMode::automatic) {
      return std::unexpected("theme base must be light or dark");
    }
    auto theme = resolve_theme(*mode);
    if (!doc.contains("colors") || !doc["colors"].is_object()) {
      return std::unexpected("theme colors must be an object");
    }
    auto colors = doc["colors"];
    constexpr std::pair<std::string_view, Color Theme::*> fields[] = {
        {"accent", &Theme::accent},     {"code", &Theme::code},
        {"add", &Theme::add},           {"del", &Theme::del},
        {"meta", &Theme::meta},         {"thinking", &Theme::thinking},
        {"error", &Theme::error},       {"muted", &Theme::muted},
        {"emphasis", &Theme::emphasis}, {"italic", &Theme::italic},
        {"quote", &Theme::quote},       {"input_fg", &Theme::input_fg},
        {"input_bg", &Theme::input_bg}, {"hover_bg", &Theme::hover_bg},
    };
    for (const auto& [key, value] : colors.items()) {
      if (!value.is_string()) {
        return std::unexpected("invalid color for " + key);
      }
      const auto hex = value.get<std::string>();
      if (hex.size() != 7 || hex[0] != '#' || !std::all_of(hex.begin() + 1, hex.end(), is_hex)) {
        return std::unexpected("color " + key + " must be #RRGGBB");
      }
      auto channel = [&](size_t offset) {
        return static_cast<uint8_t>(*hex_channel(std::string_view(hex).substr(offset, 2)));
      };
      const auto color = Color::RGB(channel(1), channel(3), channel(5));
      const auto field = std::find_if(std::begin(fields), std::end(fields),
                                      [&](const auto& item) { return item.first == key; });
      if (field == std::end(fields)) {
        return std::unexpected("unknown theme color: " + key);
      }
      theme.*(field->second) = color;
    }
    return theme;
  } catch (const std::exception& error) {
    return std::unexpected("invalid theme " + std::string(name) + ": " + error.what());
  }
}

} // namespace niminal::app
