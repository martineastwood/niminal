#include "transcript.hpp"
#include "extensions.hpp"
#include "skills.hpp"

#include <niminal/text.hpp>

#include <niminal/json.hpp>

#include <ftxui/dom/node.hpp>
#include <ftxui/screen/screen.hpp>

#include <algorithm>
#include <iterator>
#include <sstream>
#include <utility>

namespace niminal::app {

using namespace ftxui;
using json = niminal::json;

namespace {

constexpr size_t kToolResultMaxChars = 8000;
constexpr int kToolResultMaxLines = 120;
constexpr int kBashPreviewMaxLines = 8;

json parse_tool_args(const std::string& args) {
  if (args.empty()) {
    return json_object();
  }
  try {
    return json_parse(args);
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
    return niminal::clip_line(niminal::json_dump(j), 120);
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

// The bash tool appends a trailing "exit: ..." line that the collapsed preview
// clips away. Split it out so the card can show the outcome on the command line
// and the body stays free of the duplicate.
std::pair<std::string, std::string> bash_exit_status(std::string_view result) {
  const auto newline = result.rfind('\n');
  const auto line_start = newline == std::string_view::npos ? 0 : newline + 1;
  constexpr std::string_view kPrefix = "exit: ";
  if (!result.substr(line_start).starts_with(kPrefix)) {
    return {"", std::string(result)};
  }
  auto body = result.substr(0, line_start);
  if (!body.empty() && body.back() == '\n') {
    body.remove_suffix(1);
  }
  return {std::string(result.substr(line_start + kPrefix.size())), std::string(body)};
}

// Shell statuses read as codes to anyone who does not know shell conventions,
// so the card marks the outcome with an icon and keeps the code. The second
// element reports success.
std::pair<std::string, bool> bash_status(std::string_view status) {
  if (status == "0") {
    return {"✓", true};
  }
  if (status.find_first_not_of("0123456789") == std::string_view::npos) {
    return {"✗ (exit " + std::string(status) + ")", false};
  }
  if (status.starts_with("timeout")) {
    return {"timed out" + std::string(status.substr(7)), false};
  }
  return {std::string(status), false};
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
    return niminal::json_pretty(json_parse(args));
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

Elements wrapped_chunks(std::string_view word) {
  Elements chunks;
  for (auto chunk : niminal::wrap_chunks(word)) {
    chunks.push_back(text(chunk));
  }
  return chunks;
}

// Splits text into one row of chunks per source line. Spaces stay their own
// chunks so wrapping keeps them and nothing is lost between words.
std::vector<Elements> wrapped_rows(std::string_view value) {
  std::vector<Elements> rows;
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
      auto chunks = wrapped_chunks(value.substr(start, end - start));
      parts.insert(parts.end(), std::make_move_iterator(chunks.begin()),
                   std::make_move_iterator(chunks.end()));
      start = end;
    }
    if (parts.empty()) {
      parts.push_back(text(""));
    }
    rows.push_back(std::move(parts));
    if (newline == std::string_view::npos) {
      break;
    }
    line_start = newline + 1;
  }
  return rows;
}

Element wrapped_text(std::string_view value) {
  Elements rows;
  for (auto& row : wrapped_rows(value)) {
    rows.push_back(hflow(std::move(row)));
  }
  return vbox(std::move(rows));
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

bool is_conversation_block(BlockKind kind) {
  return kind == BlockKind::user || kind == BlockKind::assistant || kind == BlockKind::thinking ||
         kind == BlockKind::tool || kind == BlockKind::diff || kind == BlockKind::approval;
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
      auto [status, output] = bash_exit_status(block.result);
      auto header = wrapped_rows("$ " + bash_command(block.text));
      for (auto& row : header) {
        for (auto& chunk : row) {
          chunk |= color(theme.meta);
        }
      }
      if (!status.empty()) {
        const auto [label, ok] = bash_status(status);
        // The outcome reads as a trailing word of the command, so it stays on
        // the command line and wraps with it instead of being clipped away.
        for (auto& chunk : wrapped_chunks("  " + label)) {
          header.back().push_back(std::move(chunk) | color(ok ? theme.add : theme.del));
        }
      }
      Elements parts;
      for (auto& row : header) {
        parts.push_back(hflow(std::move(row)));
      }
      if (!output.empty()) {
        if (!block.expanded) {
          parts.push_back(
              paragraph_preserving_whitespace(take_lines(output, kBashPreviewMaxLines)) | dim);
          if (result_has_more(output, kBashPreviewMaxLines)) {
            parts.push_back(text("… click to expand") | dim);
          }
        } else {
          parts.push_back(paragraph_preserving_whitespace(output) | dim);
        }
      }
      return vbox(std::move(parts)) | reflect(box);
    }
    if (!block.expanded) {
      const auto detail = "→ " + tool_detail_line(block.tool_name, block.text);
      return paragraph_preserving_whitespace(detail) | color(theme.meta) | reflect(box);
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
      return paragraph_preserving_whitespace(header) | color(theme.muted) | reflect(box);
    }
    return vbox({paragraph_preserving_whitespace(header) | color(theme.muted),
                 render_diff_card(block, theme)}) |
           reflect(box);
  }
  default:
    return text("") | reflect(box);
  }
}

Element render_approval_block(const Block& block, const Theme& theme) {
  if (block.result == "allowed") {
    return text("✓ allowed  " + block.text) | color(theme.add);
  }
  if (block.result == "denied") {
    return text("✗ denied  " + block.text) | color(theme.del);
  }
  return text("→ approval needed  " + block.text) | color(theme.meta);
}

Element render_user_message(const Block& block, const Theme& theme) {
  auto body =
      hbox({text(" "), paragraph_preserving_whitespace(block.text) | color(theme.input_fg) | xflex,
            text(" ")});
  return hbox({filler() | bgcolor(theme.accent) | size(WIDTH, EQUAL, 1),
               vbox({text(" "), body, text(" ")}) | bgcolor(theme.input_bg) | xflex});
}

WelcomeCatalog welcome_catalog(const std::filesystem::path& cwd,
                               const ExtensionRuntime* extensions) {
  WelcomeCatalog catalog;
  for (const auto& skill : discover_skills(cwd)) {
    catalog.skills.push_back(skill.name);
  }
  if (extensions) {
    catalog.extensions = extensions->names();
    catalog.tools = extensions->tool_names();
  }
  return catalog;
}

Element render_welcome_screen(const std::string& title, const std::string& model_line,
                              const std::vector<std::string>& skills,
                              const std::vector<std::string>& extensions,
                              const std::vector<std::string>& tools, int width,
                              const Theme& theme) {
  constexpr size_t kLabelWidth = 14;
  auto hint = [&](std::string_view cmd, std::string_view desc) {
    const auto pad = static_cast<size_t>(std::max(1, 14 - static_cast<int>(cmd.size())));
    return hbox({text(std::string(cmd)) | bold | color(theme.accent), text(std::string(pad, ' ')),
                 text(std::string(desc)) | color(theme.muted)});
  };
  // Comma-separated names wrapped to the available width, so a long list stays
  // on screen instead of running past the right edge.
  auto list_rows = [&](std::string_view label, const std::vector<std::string>& values) {
    const auto span = static_cast<size_t>(std::max(1, width - static_cast<int>(kLabelWidth)));
    std::vector<std::string> lines;
    std::string current;
    for (const auto& value : values) {
      auto candidate = current.empty() ? value : current + ", " + value;
      if (!current.empty() && candidate.size() > span) {
        lines.push_back(std::move(current));
        current = value;
      } else {
        current = std::move(candidate);
      }
    }
    if (!current.empty()) {
      lines.push_back(std::move(current));
    }
    Elements rows;
    for (size_t i = 0; i < lines.size(); ++i) {
      if (i == 0) {
        rows.push_back(hint(label, lines[i]));
      } else {
        rows.push_back(
            hbox({text(std::string(kLabelWidth, ' ')), text(lines[i]) | color(theme.muted)}));
      }
    }
    return rows;
  };
  Elements rows = {text(title) | bold | color(theme.emphasis),
                   text(model_line) | bold | color(theme.accent),
                   text(""),
                   hint("/help", "list commands"),
                   hint("@file", "mention a workspace file"),
                   hint("!cmd", "run a shell command")};
  auto append_list = [&](std::string_view label, const std::vector<std::string>& values) {
    for (auto& row : list_rows(label, values)) {
      rows.push_back(std::move(row));
    }
  };
  if (!skills.empty() || !extensions.empty() || !tools.empty()) {
    rows.push_back(text(""));
    append_list("skills", skills);
    append_list("extensions", extensions);
    append_list("tools", tools);
  }
  rows.push_back(text(""));
  rows.push_back(text("describe a change in the composer below") | color(theme.muted));
  return vbox(std::move(rows));
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
    const auto type = niminal::json_value(event, "type", "");
    if (type == "user") {
      std::string text;
      if (event.contains("content") && event["content"].is_array()) {
        for (const auto& part : event["content"].get_array()) {
          if (part.is_object() && niminal::json_value(part, "type", "") == "text") {
            text += niminal::json_value(part, "text", "");
          } else if (part.is_object() && niminal::json_value(part, "type", "") == "image") {
            text += (text.empty() ? "" : "\n") + std::string("[image: ") +
                    niminal::json_value(part, "name", "image") + "]";
          }
        }
      }
      if (!text.empty()) {
        blocks.push_back(Block{BlockKind::user, std::move(text)});
      }
    } else if (type == "assistant") {
      std::string text;
      if (event.contains("content") && event["content"].is_array()) {
        for (const auto& part : event["content"].get_array()) {
          if (!part.is_object()) {
            continue;
          }
          const auto ptype = niminal::json_value(part, "type", "");
          if (ptype == "text") {
            text += niminal::json_value(part, "text", "");
          }
          if (ptype == "tool_use") {
            if (!text.empty()) {
              blocks.push_back(Block{BlockKind::assistant, text});
              text.clear();
            }
            json input = niminal::json_value(part, "input", json_object());
            Block tool{BlockKind::tool,
                       input.is_object() ? niminal::json_dump(input) : std::string()};
            tool.tool_name = niminal::json_value(part, "name", "");
            tool.tool_id = niminal::json_value(part, "id", "");
            blocks.push_back(std::move(tool));
          }
        }
      }
      if (!text.empty()) {
        blocks.push_back(Block{BlockKind::assistant, std::move(text)});
      }
    } else if (type == "bash") {
      Block tool{BlockKind::tool,
                 niminal::json_dump(json{{"command", niminal::json_value(event, "command", "")}})};
      tool.tool_name = "bash";
      tool.result = niminal::json_value(event, "output", "");
      blocks.push_back(std::move(tool));
    } else if (type == "tool_result") {
      const auto id = niminal::json_value(event, "id", "");
      const auto output = niminal::json_value(event, "output", "");
      auto it = std::find_if(blocks.rbegin(), blocks.rend(), [&](const Block& block) {
        return block.kind == BlockKind::tool && block.tool_id == id;
      });
      if (it != blocks.rend()) {
        it->result = output;
      }
    } else if (type == "compaction") {
      const auto summary = niminal::json_value(event, "summary", "");
      blocks.push_back(
          Block{BlockKind::status, "Compacted earlier turns.\n" + clip_text(summary, 1200, 12)});
    }
  }
  return blocks;
}

Element paragraph_preserving_whitespace(std::string_view value) {
  return wrapped_text(value);
}

} // namespace niminal::app
