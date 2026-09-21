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

std::string tool_detail_line(const std::string& name, const std::string& args) {
  json j = json::object();
  try {
    if (!args.empty()) {
      j = json::parse(args);
    }
  } catch (...) {
    return name + (args.empty() ? "" : "  " + one_line(args, 120));
  }
  if (!j.is_object()) {
    return name + (args.empty() ? "" : "  " + one_line(args, 120));
  }
  std::string detail;
  if (name == "bash") {
    detail =
        "$ " + (j.contains("command") && j["command"].is_string() ? j["command"].get<std::string>()
                                                                  : std::string());
  } else if (name == "skill") {
    detail =
        j.contains("name") && j["name"].is_string() ? j["name"].get<std::string>() : std::string();
  } else if (j.contains("path") && j["path"].is_string()) {
    detail = j["path"].get<std::string>();
    if (j.contains("pattern") && j["pattern"].is_string()) {
      detail += "  " + j["pattern"].get<std::string>();
    } else if (j.contains("old_text") && j["old_text"].is_string()) {
      detail += "  " + one_line(j["old_text"].get<std::string>(), 60);
    }
  } else if (j.contains("pattern") && j["pattern"].is_string()) {
    detail = j["pattern"].get<std::string>();
  } else if (!j.empty()) {
    detail = j.dump();
  }
  if (detail.empty()) {
    return name;
  }
  return name + "  " + one_line(detail, 120);
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

} // namespace

bool is_card_block(BlockKind kind) {
  return kind == BlockKind::thinking || kind == BlockKind::tool || kind == BlockKind::diff;
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
  const char* chevron = block.expanded ? "▾ " : "▸ ";

  switch (block.kind) {
  case BlockKind::thinking: {
    if (!block.expanded) {
      return text(std::string(chevron) + "thinking") | block_style(block.kind, theme) |
             reflect(box);
    }
    return vbox({text(std::string(chevron) + "thinking") | block_style(block.kind, theme),
                 paragraph_preserving_whitespace(block.text) | block_style(block.kind, theme)}) |
           reflect(box);
  }
  case BlockKind::tool: {
    if (!block.expanded) {
      return text(std::string(chevron) + tool_detail_line(block.tool_name, block.text)) |
             color(theme.meta) | reflect(box);
    }
    Elements parts;
    parts.push_back(text(std::string(chevron) + block.tool_name) | bold | color(theme.meta));
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
    if (!block.expanded) {
      return text(std::string(chevron) + "✓ " + block.tool_name + "  " + block.path) |
             color(theme.muted) | reflect(box);
    }
    return vbox({text(chevron) | dim, render_diff_card(block, theme)}) | reflect(box);
  }
  default:
    return text("") | reflect(box);
  }
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
  return "▸ " + tool_detail_line(name, args);
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
    return "you";
  case BlockKind::assistant:
    return "niminal";
  case BlockKind::tool:
    return "";
  case BlockKind::thinking:
    return "";
  case BlockKind::diff:
    return "";
  case BlockKind::error:
    return "error";
  case BlockKind::status:
    return "";
  case BlockKind::approval:
    return "";
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
