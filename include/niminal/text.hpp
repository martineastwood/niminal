#pragma once

#include <cctype>
#include <string>
#include <string_view>

namespace niminal {

inline constexpr size_t kMaxToolContextBytes = 8'000;

inline std::string tool_context_text(std::string_view output) {
  if (output.size() <= kMaxToolContextBytes) {
    return std::string(output);
  }
  return std::string(output.substr(0, kMaxToolContextBytes)) + "\n[truncated]";
}

inline std::string lower_copy(std::string value) {
  for (char& c : value)
    c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
  return value;
}

inline std::string clip_line(std::string s, size_t max_len = 60) {
  for (char& c : s) {
    if (c == '\n' || c == '\r' || c == '\t') {
      c = ' ';
    }
  }
  if (s.size() > max_len) {
    s.resize(max_len);
    s += "…";
  }
  return s;
}

inline std::string trim_copy(std::string value) {
  while (!value.empty() && (value.back() == ' ' || value.back() == '\n' || value.back() == '\r' ||
                            value.back() == '\t')) {
    value.pop_back();
  }
  size_t i = 0;
  while (i < value.size() &&
         (value[i] == ' ' || value[i] == '\n' || value[i] == '\r' || value[i] == '\t')) {
    ++i;
  }
  value.erase(0, i);
  return value;
}

inline std::string base64_encode(std::string_view bytes) {
  constexpr std::string_view alphabet =
      "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  std::string out;
  out.reserve(((bytes.size() + 2) / 3) * 4);
  for (size_t i = 0; i < bytes.size(); i += 3) {
    const auto a = static_cast<unsigned int>(static_cast<unsigned char>(bytes[i]));
    const auto b = i + 1 < bytes.size()
                       ? static_cast<unsigned int>(static_cast<unsigned char>(bytes[i + 1]))
                       : 0U;
    const auto c = i + 2 < bytes.size()
                       ? static_cast<unsigned int>(static_cast<unsigned char>(bytes[i + 2]))
                       : 0U;
    out.push_back(alphabet[a >> 2]);
    out.push_back(alphabet[((a & 3U) << 4U) | (b >> 4U)]);
    out.push_back(i + 1 < bytes.size() ? alphabet[((b & 15U) << 2U) | (c >> 6U)] : '=');
    out.push_back(i + 2 < bytes.size() ? alphabet[c & 63U] : '=');
  }
  return out;
}

} // namespace niminal
