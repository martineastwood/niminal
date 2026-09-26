#include "transcript.hpp"

#include <niminal/text.hpp>

#include <nlohmann/json.hpp>

#include <ftxui/dom/node.hpp>
#include <ftxui/screen/screen.hpp>

#include <algorithm>
#include <sstream>
#include <utility>

namespace niminal::app {

using namespace ftxui;
using json = nlohmann::json;

namespace {

constexpr size_t kToolResultMaxChars = 8000;
constexpr int kToolResultMaxLines = 120;
constexpr int kBashPreviewMaxLines = 8;

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
      detail += "  " + niminal::clip_line(j["old_text"].get<std::string>());
    }
    return detail;
  }
  if (j.contains("pattern") && j["pattern"].is_string()) {
    return j["pattern"].get<std::string>();
  }
  if (!j.empty()) {
    return niminal::clip_line(j.dump(), 120);
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
    return name + (args.empty() ? "" : "  " + niminal::clip_line(args, 120));
  }
  const auto target = tool_target(name, j);
  if (target.empty()) {
    return name;
  }
  return name + "  " + niminal::clip_line(target, 120);
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

class VirtualTranscript : public Node {
public:
  VirtualTranscript(Elements entries, const std::vector<int>& heights)
      : entries_(std::move(entries)), laid_out_(entries_.size(), false) {
    offsets_.reserve(heights.size() + 1);
    offsets_.push_back(0);
    for (int height : heights) {
      offsets_.push_back(offsets_.back() + std::max(1, height));
    }
  }

  void ComputeRequirement() override {
    requirement_ = Requirement{};
    requirement_.min_y = offsets_.back();
  }

  void Check(Status* status) override { status->need_iteration |= status->iteration == 0; }

  void Select(Selection& selection) override {
    // Layout before Select so text nodes can record selection rows. Render must
    // not call ComputeRequirement again on those nodes: FTXUI clears selection
    // style state inside ComputeRequirement.
    std::fill(laid_out_.begin(), laid_out_.end(), false);
    prepare(selection.GetBox());
    select_prepared_ = true;
    Node::Select(selection);
  }

  void Render(Screen& screen) override {
    if (!select_prepared_) {
      std::fill(laid_out_.begin(), laid_out_.end(), false);
    }
    prepare(screen.stencil);
    select_prepared_ = false;
    Node::Render(screen);
  }

private:
  void prepare(Box area) {
    children_.clear();
    const auto visible = Box::Intersection(box_, area);
    if (visible.IsEmpty()) {
      return;
    }
    const int first_row = visible.y_min - box_.y_min;
    const int last_row = visible.y_max - box_.y_min;
    const auto first = static_cast<size_t>(
        std::upper_bound(offsets_.begin(), offsets_.end(), first_row) - offsets_.begin() - 1);
    for (size_t i = first; i < entries_.size() && offsets_[i] <= last_row; ++i) {
      auto& entry = entries_[i];
      const Box entry_box{box_.x_min, box_.x_max, box_.y_min + offsets_[i],
                          box_.y_min + offsets_[i + 1] - 1};
      if (!laid_out_[i]) {
        Status status;
        entry->Check(&status);
        while (status.need_iteration && status.iteration < 20) {
          entry->ComputeRequirement();
          entry->SetBox(entry_box);
          status.need_iteration = false;
          ++status.iteration;
          entry->Check(&status);
        }
        laid_out_[i] = true;
      } else {
        entry->SetBox(entry_box);
      }
      children_.push_back(entry);
    }
  }

  Elements entries_;
  std::vector<int> offsets_;
  std::vector<char> laid_out_;
  bool select_prepared_ = false;
};

} // namespace

bool is_card_block(BlockKind kind) {
  return kind == BlockKind::thinking || kind == BlockKind::tool || kind == BlockKind::diff;
}

bool is_drag_gesture(int press_x, int press_y, int release_x, int release_y) {
  return press_x != release_x || press_y != release_y;
}

int measure_transcript_height(const Element& element, int width) {
  Screen screen(std::max(1, width), 1);
  Render(screen, element);
  return std::max(1, element->requirement().min_y);
}

Element virtual_transcript(Elements entries, const std::vector<int>& heights) {
  return std::make_shared<VirtualTranscript>(std::move(entries), heights);
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
  // "▌" is a full-height, half-width block: a thinner rail than a solid fill.
  return hbox({text("▌") | color(theme.accent),
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

std::vector<Block> blocks_from_events(const std::vector<json>& events) {
  std::vector<Block> blocks;
  for (const auto& event : events) {
    if (!event.is_object()) {
      continue;
    }
    const auto type = event.value("type", "");
    if (type == "user") {
      std::string text;
      if (event.contains("content") && event["content"].is_array()) {
        for (const auto& part : event["content"]) {
          if (part.is_object() && part.value("type", "") == "text") {
            text += part.value("text", "");
          } else if (part.is_object() && part.value("type", "") == "image") {
            text += (text.empty() ? "" : "\n") + std::string("[image: ") +
                    part.value("name", "image") + "]";
          }
        }
      }
      if (!text.empty()) {
        blocks.push_back(Block{BlockKind::user, std::move(text)});
      }
    } else if (type == "assistant") {
      std::string text;
      if (event.contains("content") && event["content"].is_array()) {
        for (const auto& part : event["content"]) {
          if (!part.is_object()) {
            continue;
          }
          const auto ptype = part.value("type", "");
          if (ptype == "text") {
            text += part.value("text", "");
          }
          if (ptype == "tool_use") {
            if (!text.empty()) {
              blocks.push_back(Block{BlockKind::assistant, text});
              text.clear();
            }
            json input = part.value("input", json::object());
            Block tool{BlockKind::tool, input.is_object() ? input.dump() : std::string()};
            tool.tool_name = part.value("name", "");
            tool.tool_id = part.value("id", "");
            blocks.push_back(std::move(tool));
          }
        }
      }
      if (!text.empty()) {
        blocks.push_back(Block{BlockKind::assistant, std::move(text)});
      }
    } else if (type == "bash") {
      Block tool{BlockKind::tool, json{{"command", event.value("command", "")}}.dump()};
      tool.tool_name = "bash";
      tool.result = event.value("output", "");
      blocks.push_back(std::move(tool));
    } else if (type == "tool_result") {
      const auto id = event.value("id", "");
      const auto output = event.value("output", "");
      auto it = std::find_if(blocks.rbegin(), blocks.rend(), [&](const Block& block) {
        return block.kind == BlockKind::tool && block.tool_id == id;
      });
      if (it != blocks.rend()) {
        it->result = output;
      }
    } else if (type == "compaction") {
      const auto summary = event.value("summary", "");
      blocks.push_back(
          Block{BlockKind::status, "Compacted earlier turns.\n" + clip_text(summary, 1200, 12)});
    }
  }
  return blocks;
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
