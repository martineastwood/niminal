#include "diff.hpp"

#include <algorithm>
#include <sstream>
#include <utility>
#include <vector>

namespace niminal::app {
namespace {

std::vector<std::string> lines(std::string_view text) {
  if (text.empty()) return {};
  std::vector<std::string> out;
  size_t start = 0;
  while (start < text.size()) {
    auto end = text.find('\n', start);
    if (end == std::string_view::npos) end = text.size();
    auto line = std::string(text.substr(start, end - start));
    if (!line.empty() && line.back() == '\r') line.pop_back();
    out.push_back(std::move(line));
    if (end == text.size()) break;
    start = end + 1;
  }
  return out;
}

struct DiffLine {
  char prefix = ' ';
  std::string text;
  bool changed = false;
};

std::vector<DiffLine> diff_lines(const std::vector<std::string>& before,
                                 const std::vector<std::string>& after) {
  constexpr size_t kMaxCells = 4'000'000;
  if (before.size() > 2'000 || after.size() > 2'000 ||
      before.size() * after.size() > kMaxCells)
    return {{'!', "diff too large to display", true}};

  std::vector<std::vector<int>> common(
      before.size() + 1, std::vector<int>(after.size() + 1));
  for (size_t i = before.size(); i-- > 0;) {
    for (size_t j = after.size(); j-- > 0;) {
      common[i][j] = before[i] == after[j]
                         ? common[i + 1][j + 1] + 1
                         : std::max(common[i + 1][j], common[i][j + 1]);
    }
  }

  std::vector<DiffLine> out;
  size_t i = 0;
  size_t j = 0;
  while (i < before.size() || j < after.size()) {
    if (i < before.size() && j < after.size() && before[i] == after[j]) {
      out.push_back({' ', before[i++], false});
    } else if (j == after.size() ||
               (i < before.size() && common[i + 1][j] >= common[i][j + 1])) {
      out.push_back({'-', before[i++], true});
    } else {
      out.push_back({'+', after[j++], true});
    }
  }
  return out;
}

}  // namespace

FileDiff make_file_diff(bool before_exists, std::string_view before,
                        bool after_exists, std::string_view after) {
  if (before_exists == after_exists && before == after) return {};

  FileDiff result;
  result.changed = true;
  result.created = !before_exists && after_exists;

  auto old_lines = lines(before);
  auto new_lines = lines(after);
  auto all = diff_lines(old_lines, new_lines);

  std::vector<std::pair<size_t, size_t>> ranges;
  for (size_t i = 0; i < all.size();) {
    if (!all[i].changed) {
      ++i;
      continue;
    }
    auto start = i > 2 ? i - 2 : 0;
    auto end = std::min(all.size(), i + 3);
    while (i + 1 < all.size() && all[i + 1].changed) ++i;
    end = std::min(all.size(), i + 3);
    if (!ranges.empty() && start <= ranges.back().second)
      ranges.back().second = end;
    else
      ranges.push_back({start, end});
    ++i;
  }

  std::ostringstream out;
  out << "@@\n";
  if (ranges.empty()) {
    out << "  " << (after_exists ? "empty file" : "file removed") << '\n';
  } else {
    for (size_t r = 0; r < ranges.size(); ++r) {
      if (r) out << "…\n";
      for (size_t i = ranges[r].first; i < ranges[r].second; ++i)
        out << all[i].prefix << all[i].text << '\n';
    }
  }
  result.body = out.str();
  return result;
}

}  // namespace niminal::app
