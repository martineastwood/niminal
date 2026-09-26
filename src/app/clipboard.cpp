#include "clipboard.hpp"
#include "images.hpp"

#include <niminal/text.hpp>

#include <cstdio>
#include <iostream>
#include <string_view>

namespace niminal::app {
#if defined(__APPLE__)
std::string macos_clipboard_image();
#endif
namespace {

bool pipe_copy(const char* cmd, const std::string& text) {
  FILE* pipe = popen(cmd, "w");
  if (pipe == nullptr) {
    return false;
  }
  if (!text.empty()) {
    fwrite(text.data(), 1, text.size(), pipe);
  }
  return pclose(pipe) == 0;
}

std::string pipe_read(const char* cmd) {
  FILE* pipe = popen(cmd, "r");
  if (pipe == nullptr) {
    return {};
  }
  std::string out;
  char buf[4096];
  while (true) {
    auto n = fread(buf, 1, sizeof(buf), pipe);
    if (n == 0) {
      break;
    }
    out.append(buf, n);
  }
  pclose(pipe);
  return out;
}

#if defined(__linux__)
std::string pipe_read_first(std::initializer_list<const char*> commands) {
  for (const char* cmd : commands) {
    const auto out = pipe_read(cmd);
    if (!out.empty()) {
      return out;
    }
  }
  return {};
}

std::string linux_clipboard_image_bytes() {
  for (const char* type : {"image/png", "image/jpeg", "image/webp"}) {
    const std::string wl = "wl-paste -n --type " + std::string(type) + " 2>/dev/null";
    const std::string xc = "xclip -selection clipboard -t " + std::string(type) + " -o 2>/dev/null";
    auto bytes = pipe_read(wl.c_str());
    if (bytes.empty()) {
      bytes = pipe_read(xc.c_str());
    }
    if (!bytes.empty()) {
      return bytes;
    }
  }
  return {};
}
#endif

} // namespace

void copy_to_clipboard(const std::string& text) {
#if defined(__APPLE__)
  pipe_copy("pbcopy", text);
#elif defined(__linux__)
  if (!pipe_copy("wl-copy", text)) {
    pipe_copy("xclip -selection clipboard", text);
  }
#endif
  std::cout << "\033]52;c;" << niminal::base64_encode(text) << "\a" << std::flush;
}

std::string paste_from_clipboard() {
  std::string text;
#if defined(__APPLE__)
  text = pipe_read("pbpaste");
#elif defined(__linux__)
  text = pipe_read_first({"wl-paste -n 2>/dev/null", "xclip -selection clipboard -o 2>/dev/null"});
#endif
  for (auto& c : text) {
    if (c == '\r') {
      c = '\n';
    }
  }
  return text;
}

std::optional<niminal::json> paste_image_from_clipboard() {
  std::string bytes;
#if defined(__APPLE__)
  bytes = macos_clipboard_image();
#elif defined(__linux__)
  bytes = linux_clipboard_image_bytes();
#endif
  if (bytes.empty()) {
    return std::nullopt;
  }
  return clipboard_image_part(bytes);
}

} // namespace niminal::app
