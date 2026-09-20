#include "diff.hpp"

#include <algorithm>
#include <charconv>
#include <sstream>
#include <string_view>
#include <vector>

namespace niminal::app {
namespace {

struct HunkSpan {
  int old_start = 0;
  int new_start = 0;
};

struct HunkEntry {
  bool removed = false;
  int number = 0;
  std::string text;
};

std::vector<std::string> lines(std::string_view text) {
  if (text.empty()) return {};
  std::vector<std::string> result;
  size_t start = 0;
  while (start < text.size()) {
    auto end = text.find('\n', start);
    if (end == std::string_view::npos) end = text.size();
    result.emplace_back(text.substr(start, end - start));
    if (!result.back().empty() && result.back().back() == '\r')
      result.back().pop_back();
    if (end == text.size()) break;
    start = end + 1;
  }
  return result;
}

int parse_number(std::string_view text) {
  int number = 0;
  auto [end, error] =
      std::from_chars(text.data(), text.data() + text.size(), number);
  return error == std::errc{} && end == text.data() + text.size() ? number : 0;
}

HunkSpan parse_span(std::string_view text) {
  const auto arrow = text.find(" > ");
  if (arrow == std::string_view::npos) return {};
  const auto old_range = text.substr(0, arrow);
  const auto new_range = text.substr(arrow + 3);
  const auto old_dash = old_range.find('-');
  const auto new_dash = new_range.find('-');
  if (old_dash == std::string_view::npos || new_dash == std::string_view::npos)
    return {};
  return {parse_number(old_range.substr(0, old_dash)),
          parse_number(new_range.substr(0, new_dash))};
}

std::vector<HunkSpan> parse_spans(std::string_view output) {
  std::vector<HunkSpan> result;
  for (const auto& line : lines(output)) {
    if (!line.starts_with("lines: ")) continue;
    std::string_view rest = line;
    rest.remove_prefix(7);
    while (!rest.empty()) {
      const auto comma = rest.find(", ");
      const auto part = rest.substr(0, comma);
      auto span = parse_span(part);
      if (span.old_start > 0 && span.new_start > 0)
        result.push_back(span);
      if (comma == std::string_view::npos) break;
      rest.remove_prefix(comma + 2);
    }
  }
  return result;
}

std::string value(const nlohmann::json& input, std::string_view key) {
  if (!input.is_object()) return {};
  const auto it = input.find(key);
  return it != input.end() && it->is_string() ? it->get<std::string>() : "";
}

void add_pair(std::vector<HunkEntry>& entries, std::string_view old_text,
              std::string_view new_text, const HunkSpan& span) {
  if (!old_text.empty()) {
    int number = span.old_start;
    for (const auto& line : lines(old_text))
      entries.push_back({true, number > 0 ? number++ : 0, line});
  }
  if (!new_text.empty()) {
    int number = span.new_start;
    for (const auto& line : lines(new_text))
      entries.push_back({false, number > 0 ? number++ : 0, line});
  }
}

std::string format_entry(const HunkEntry& entry, int width) {
  std::ostringstream out;
  out << (entry.removed ? "- " : "+ ");
  if (entry.number > 0) {
    out.width(width);
    out << entry.number << " | ";
  }
  out << entry.text;
  return out.str();
}

}  // namespace

FileDiff make_tool_diff(std::string_view tool_name, const nlohmann::json& input,
                        bool created, std::string_view output) {
  if (!input.is_object()) return {};

  std::vector<HunkEntry> entries;
  const auto spans = parse_spans(output);
  if (tool_name == "edit") {
    const auto replacements = input.find("replacements");
    if (replacements != input.end() && replacements->is_array() &&
        !replacements->empty()) {
      for (size_t i = 0; i < replacements->size(); ++i) {
        const auto& replacement = (*replacements)[i];
        if (!replacement.is_object()) continue;
        const HunkSpan span = i < spans.size() ? spans[i] : HunkSpan{};
        add_pair(entries, value(replacement, "old_text"),
                 value(replacement, "new_text"), span);
      }
    } else {
      add_pair(entries, value(input, "old_text"), value(input, "new_text"),
               spans.empty() ? HunkSpan{} : spans[0]);
    }
  } else if (tool_name == "write") {
    int number = 1;
    for (const auto& line : lines(value(input, "content")))
      entries.push_back({false, number++, line});
  } else {
    return {};
  }

  if (entries.empty()) return {};
  int width = 0;
  for (const auto& entry : entries)
    if (entry.number > 0)
      width = std::max(width,
                       static_cast<int>(std::to_string(entry.number).size()));

  FileDiff result;
  result.changed = true;
  result.created = created;
  for (const auto& entry : entries)
    result.body += format_entry(entry, width) + '\n';
  return result;
}

}  // namespace niminal::app
