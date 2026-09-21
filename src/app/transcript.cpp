#include "transcript.hpp"

#include <nlohmann/json.hpp>

#include <sstream>
#include <utility>

namespace niminal::app {

using namespace ftxui;
using json = nlohmann::json;

namespace {

constexpr size_t kToolResultMaxChars = 8000;
constexpr int kToolResultMaxLines = 120;
constexpr int kBashPreviewMaxLines = 8;

std::string one_line(std::string s, size_t n) {
  for (char& c : s) {
    if (c == '\n' || c == '\r' || c == '\t') {
      c = ' ';
    }
  }
  if (s.size() > n) {
    s.resize(n);
    s += "…";
  }
  return s;
}

json parse_tool_args(const std::string& args) {
  if (args.empty()) {
    return json::object();
  }
  try {
    return json::parse(args);
  } catch (...) {
    return json();
  }
}

std::string tool_target(const std::string& name, const json& j) {
  if (!j.is_object()) {
    return {};
  }
  if (name == "skill") {
    return j.contains("name") && j["name"].is_string() ? j["name"].get<std::string>()
                                                       : std::string();
  }
  if (j.contains("path") && j["path"].is_string()) {
    std::string detail = j["path"].get<std::string>();
    if (j.contains("pattern") && j["pattern"].is_string()) {
      detail += "  " + j["pattern"].get<std::string>();
    } else if (j.contains("old_text") && j["old_text"].is_string()) {
      detail += "  " + one_line(j["old_text"].get<std::string>(), 60);
    }
    return detail;
  }
  if (j.contains("pattern") && j["pattern"].is_string()) {
    return j["pattern"].get<std::string>();
  }
  if (!j.empty()) {
    return one_line(j.dump(), 120);
  }
  return {};
}

std::string bash_command(const std::string& args) {
  const auto j = parse_tool_args(args);
  if (j.is_object() && j.contains("command") && j["command"].is_string()) {
    return j["command"].get<std::string>();
  }
  return {};
}

std::string tool_detail_line(const std::string& name, const std::string& args) {
  const auto j = parse_tool_args(args);
  if (!j.is_object()) {
    return name + (args.empty() ? "" : "  " + one_line(args, 120));
  }
  const auto target = tool_target(name, j);
  if (target.empty()) {
    return name;
  }
  return name + "  " + one_line(target, 120);
}

std::string pretty_tool_args(const std::string& args) {
  if (args.empty()) {
    return "{}";
  }
  try {
    return json::parse(args).dump(2);
  } catch (...) {
    return args;
  }
}

int count_lines(std::string_view text) {
  int lines = text.empty() ? 0 : 1;
  for (char c : text) {
    if (c == '\n') {
      ++lines;
    }
  }
  return lines;
}

std::string take_lines(std::string_view text, int max_lines) {
  if (max_lines <= 0 || text.empty()) {
    return {};
  }
  int kept = 1;
  std::string out;
  for (char c : text) {
    out += c;
    if (c == '\n' && ++kept > max_lines) {
      break;
    }
  }
  while (!out.empty() && out.back() == '\n') {
    out.pop_back();
  }
  return out;
}

bool result_has_more(const std::string& result, int preview_lines) {
  return count_lines(result) > preview_lines || result.size() > kToolResultMaxChars;
}

} // namespace

bool is_card_block(BlockKind kind) {
  return kind == BlockKind::thinking || kind == BlockKind::tool || kind == BlockKind::diff;
}

bool is_drag_gesture(int press_x, int press_y, int release_x, int release_y) {
  return press_x != release_x || press_y != release_y;
}

Element render_diff_card(const Block& block, const Theme& theme) {
  Elements lines;
  std::istringstream in(block.text);
  std::string line;
  while (std::getline(in, line)) {
    Element row = text("│   " + line);
    if (!line.empty() && line.front() == '+') {
      row = row | color(theme.add);
    } else if (!line.empty() && line.front() == '-') {
      row = row | color(theme.del);
    } else {
      row = row | dim;
    }
    lines.push_back(std::move(row));
  }
  auto badge = text("│ ✓ " + block.tool_name) | bold | color(theme.add);
  return vbox({badge, text("│   " + block.path) | dim, vbox(std::move(lines))});
}

Element render_transcript_card(const Block& block, const Theme& theme, Box& box) {
  switch (block.kind) {
  case BlockKind::thinking: {
    const char* prefix = block.expanded ? "- Thought" : "+ Thought";
    if (!block.expanded) {
      return text(prefix) | color(theme.thinking) | reflect(box);
    }
    return vbox({text(prefix) | color(theme.thinking),
                 paragraph_preserving_whitespace(block.text) | dim | italic}) |
           reflect(box);
  }
  case BlockKind::tool: {
    if (block.tool_name == "bash") {
      const std::string command = bash_command(block.text);
      Elements parts;
      parts.push_back(text("$ " + command) | color(theme.meta));
      if (!block.result.empty()) {
        if (!block.expanded) {
          parts.push_back(
              paragraph_preserving_whitespace(take_lines(block.result, kBashPreviewMaxLines)) |
              dim);
          if (result_has_more(block.result, kBashPreviewMaxLines)) {
            parts.push_back(text("… click to expand") | dim);
          }
        } else {
          parts.push_back(paragraph_preserving_whitespace(block.result) | dim);
        }
      }
      return vbox(std::move(parts)) | reflect(box);
    }
    if (!block.expanded) {
      return text("→ " + tool_detail_line(block.tool_name, block.text)) | color(theme.meta) |
             reflect(box);
    }
    Elements parts;
    parts.push_back(text("→ " + block.tool_name) | bold | color(theme.meta));
    parts.push_back(text("arguments") | dim);
    parts.push_back(paragraph_preserving_whitespace(pretty_tool_args(block.text)) |
                    color(theme.meta));
    if (!block.result.empty()) {
      parts.push_back(text(""));
      parts.push_back(text("result") | dim);
      parts.push_back(paragraph_preserving_whitespace(
                          clip_text(block.result, kToolResultMaxChars, kToolResultMaxLines)) |
                      dim);
    }
    return vbox(std::move(parts)) | reflect(box);
  }
  case BlockKind::diff: {
    const auto header = "→ ✓ " + block.tool_name + "  " + block.path;
    if (!block.expanded) {
      return text(header) | color(theme.muted) | reflect(box);
    }
    return vbox({text(header) | color(theme.muted), render_diff_card(block, theme)}) | reflect(box);
  }
  default:
    return text("") | reflect(box);
  }
}

Element render_user_message(const Block& block, const Theme& theme) {
  auto body =
      hbox({text(" "), paragraph_preserving_whitespace(block.text) | color(theme.input_fg) | xflex,
            text(" ")});
  return hbox({filler() | bgcolor(theme.accent) | size(WIDTH, EQUAL, 1),
               vbox({text(" "), body, text(" ")}) | bgcolor(theme.input_bg) | xflex});
}

std::string clip_text(std::string text, size_t max_chars, int max_lines) {
  int lines = 1;
  for (char c : text) {
    if (c == '\n') {
      ++lines;
    }
  }
  if (text.size() > max_chars) {
    text.resize(max_chars);
    text += "\n[truncated]";
    return text;
  }
  if (lines <= max_lines) {
    return text;
  }
  std::string out;
  int kept = 0;
  for (char c : text) {
    out += c;
    if (c == '\n' && ++kept >= max_lines) {
      break;
    }
  }
  out += "[truncated]";
  return out;
}

std::string tool_summary(const std::string& name, const std::string& args) {
  if (name == "bash") {
    return "$ " + bash_command(args);
  }
  return "→ " + tool_detail_line(name, args);
}

Decorator block_style(BlockKind kind, const Theme& theme) {
  switch (kind) {
  case BlockKind::user:
    return color(theme.accent);
  case BlockKind::tool:
    return color(theme.meta);
  case BlockKind::thinking:
    return [](Element e) { return std::move(e) | dim | italic; };
  case BlockKind::diff:
    return color(theme.muted);
  case BlockKind::error:
    return color(theme.error);
  case BlockKind::status:
    return color(theme.muted);
  case BlockKind::approval:
    return color(theme.meta);
  case BlockKind::assistant:
    return Decorator([](Element e) { return e; });
  }
  return Decorator([](Element e) { return e; });
}

const char* block_label(BlockKind kind) {
  switch (kind) {
  case BlockKind::user:
  case BlockKind::assistant:
  case BlockKind::tool:
  case BlockKind::thinking:
  case BlockKind::diff:
  case BlockKind::status:
  case BlockKind::approval:
    return "";
  case BlockKind::error:
    return "error";
  }
  return "";
}

Element paragraph_preserving_whitespace(std::string_view value) {
  Elements rows;
  size_t line_start = 0;
  while (true) {
    const auto newline = value.find('\n', line_start);
    const auto line_end = newline == std::string_view::npos ? value.size() : newline;
    Elements parts;
    size_t start = line_start;
    while (start < line_end) {
      size_t end = start + 1;
      if (value[start] == ' ') {
        while (end < line_end && value[end] == ' ') {
          ++end;
        }
      } else {
        while (end < line_end && value[end] != ' ') {
          ++end;
        }
      }
      parts.push_back(text(value.substr(start, end - start)));
      start = end;
    }
    if (parts.empty()) {
      parts.push_back(text(""));
    }
    rows.push_back(hflow(std::move(parts)));
    if (newline == std::string_view::npos) {
      break;
    }
    line_start = newline + 1;
  }
  return vbox(std::move(rows));
}

} // namespace niminal::app
