#include "clipboard.hpp"

#include <cstdio>
#include <iostream>
#include <string_view>

namespace niminal::app {
namespace {

std::string base64_encode(std::string_view in) {
  static constexpr char kTbl[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  std::string out;
  int val = 0;
  int valb = -6;
  for (char ch : in) {
    const unsigned char c = static_cast<unsigned char>(ch);
    val = (val << 8) + c;
    valb += 8;
    while (valb >= 0) {
      out.push_back(kTbl[(val >> valb) & 0x3F]);
      valb -= 6;
    }
  }
  if (valb > -6) {
    out.push_back(kTbl[((val << 8) >> (valb + 8)) & 0x3F]);
  }
  while ((out.size() % 4) != 0U) {
    out.push_back('=');
  }
  return out;
}

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

} // namespace

void copy_to_clipboard(const std::string& text) {
#if defined(__APPLE__)
  pipe_copy("pbcopy", text);
#elif defined(__linux__)
  if (!pipe_copy("wl-copy", text)) {
    pipe_copy("xclip -selection clipboard", text);
  }
#endif
  std::cout << "\033]52;c;" << base64_encode(text) << "\a" << std::flush;
}

std::string paste_from_clipboard() {
  std::string text;
#if defined(__APPLE__)
  text = pipe_read("pbpaste");
#elif defined(__linux__)
  text = pipe_read("wl-paste -n 2>/dev/null");
  if (text.empty()) {
    text = pipe_read("xclip -selection clipboard -o 2>/dev/null");
  }
#endif
  for (auto& c : text) {
    if (c == '\r') {
      c = '\n';
    }
  }
  return text;
}

} // namespace niminal::app
