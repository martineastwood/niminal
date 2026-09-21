#include "transcript.hpp"

#include <nlohmann/json.hpp>

#include <sstream>
#include <utility>

namespace niminal::app {

using namespace ftxui;
using json = nlohmann::json;

namespace {

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

} // namespace

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
  json j = json::object();
  try {
    if (!args.empty()) {
      j = json::parse(args);
    }
  } catch (...) {
    return "▸ " + name + "  " + one_line(args, 120);
  }
  if (!j.is_object()) {
    return "▸ " + name + (args.empty() ? "" : "  " + one_line(args, 120));
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
    return "▸ " + name;
  }
  return "▸ " + name + "  " + one_line(detail, 120);
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
