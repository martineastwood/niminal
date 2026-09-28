#pragma once

#include <algorithm>
#include <cctype>
#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace niminal {

inline constexpr size_t kMaxToolContextBytes = 8'000;

// A terminal row can only break between elements, so a word wider than the row
// runs off the right edge instead of wrapping. Words longer than
// kBreakWordBytes are split into pieces of at most kWrapChunkBytes, which gives
// the row somewhere to break. Bytes are never fewer than the columns a string
// occupies, so a piece never exceeds its byte budget on screen.
inline constexpr size_t kBreakWordBytes = 24;
inline constexpr size_t kWrapChunkBytes = 8;

inline std::vector<std::string_view> wrap_chunks(std::string_view word) {
  if (word.size() <= kBreakWordBytes) {
    return {word};
  }
  std::vector<std::string_view> chunks;
  size_t start = 0;
  while (start < word.size()) {
    size_t end = std::min(start + kWrapChunkBytes, word.size());
    while (end < word.size() && (static_cast<unsigned char>(word[end]) & 0xC0U) == 0x80U) {
      ++end;
    }
    chunks.push_back(word.substr(start, end - start));
    start = end;
  }
  return chunks;
}

inline std::string tool_context_text(std::string_view output) {
  std::string text = output.size() <= kMaxToolContextBytes
                         ? std::string(output)
                         : std::string(output.substr(0, kMaxToolContextBytes)) + "\n[truncated]";
  std::erase(text, '\0');
  return text;
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

inline std::optional<std::string> base64_decode(std::string_view encoded) {
  constexpr std::string_view alphabet =
      "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  if (encoded.size() % 4 != 0)
    return std::nullopt;
  std::string bytes;
  bytes.reserve((encoded.size() / 4) * 3);
  for (size_t i = 0; i < encoded.size(); i += 4) {
    std::uint32_t value = 0;
    unsigned int padding = 0;
    for (size_t j = 0; j < 4; ++j) {
      const char character = encoded[i + j];
      if (character == '=') {
        ++padding;
        value <<= 6U;
        continue;
      }
      if (padding != 0)
        return std::nullopt;
      const auto index = alphabet.find(character);
      if (index == std::string_view::npos)
        return std::nullopt;
      value = (value << 6U) | static_cast<std::uint32_t>(index);
    }
    bytes.push_back(static_cast<char>((value >> 16U) & 0xffU));
    if (padding < 2U) {
      bytes.push_back(static_cast<char>((value >> 8U) & 0xffU));
    }
    if (padding < 1U) {
      bytes.push_back(static_cast<char>(value & 0xffU));
    }
  }
  return bytes;
}

} // namespace niminal
