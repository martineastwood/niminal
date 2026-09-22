#include "session.hpp"

#include <algorithm>
#include <niminal/text.hpp>

#include <niminal/agent.hpp>

#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <sstream>
#include <stdexcept>
#include <unistd.h>
#include <vector>

namespace niminal::app {
namespace fs = std::filesystem;
using json = nlohmann::json;

namespace {

void fsync_file(const fs::path& path) {
  FILE* f = std::fopen(path.c_str(), "ab");
  if (f == nullptr) {
    return;
  }
  std::fflush(f);
  fsync(fileno(f));
  std::fclose(f);
}

std::string clip_line(std::string s, size_t n = 60) {
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

std::string first_user_text(const json& event) {
  if (event.value("type", "") != "user") {
    return {};
  }
  if (!event.contains("content") || !event["content"].is_array()) {
    return {};
  }
  for (const auto& part : event["content"]) {
    if (part.value("type", "") == "text") {
      return part.value("text", "");
    }
    if (part.value("type", "") == "image") {
      return "[image: " + part.value("name", "image") + "]";
    }
  }
  return {};
}

json text_block(const std::string& text) {
  return json{{"type", "text"}, {"text", text}};
}

std::string event_text(const json& event) {
  std::string text;
  if (event.contains("content") && event["content"].is_array()) {
    for (const auto& part : event["content"]) {
      if (part.is_object() && part.value("type", "") == "text") {
        text += part.value("text", "");
      } else if (part.is_object() && part.value("type", "") == "image") {
        text += "[image: " + part.value("name", "image") + "]";
      }
    }
  }
  return text;
}

std::string bash_user_message(const std::string& command, const std::string& output) {
  std::string text = "Ran `" + command + "`\n";
  if (output.empty()) {
    text += "(no output)";
  } else {
    text += "```\n" + output + "\n```";
  }
  return text;
}

bool session_matches(const Session& session, const std::string& query) {
  auto needle = niminal::lower_copy(query);
  if (needle.empty()) {
    return true;
  }
  std::string haystack = niminal::lower_copy(session.name) + '\n' +
                         niminal::lower_copy(session.workspace) + '\n' + session.id;
  for (const auto& event : session.events) {
    if (!event.is_object()) {
      continue;
    }
    auto type = event.value("type", "");
    if (type == "user" || type == "assistant") {
      haystack += '\n' + niminal::lower_copy(event_text(event));
    } else if (type == "bash") {
      haystack += '\n' + niminal::lower_copy(event.value("command", ""));
      haystack += '\n' + niminal::lower_copy(event.value("output", ""));
    } else if (type == "tool_result") {
      haystack += '\n' + niminal::lower_copy(event.value("output", ""));
    } else if (type == "compaction") {
      haystack += '\n' + niminal::lower_copy(event.value("summary", ""));
    }
  }
  return haystack.find(needle) != std::string::npos;
}

fs::path trash_dir(const fs::path& dir) {
  return dir / ".trash";
}

std::vector<SessionInfo> collect_sessions(const fs::path& dir, const std::string& workspace,
                                          int limit, const std::string& query, bool deleted) {
  std::vector<SessionInfo> all;
  std::error_code ec;
  if (!fs::exists(dir, ec)) {
    return {};
  }
  for (const auto& entry : fs::directory_iterator(dir, ec)) {
    if (!entry.is_regular_file()) {
      continue;
    }
    auto name = entry.path().filename().string();
    if (name.size() < 6 || !name.ends_with(".jsonl")) {
      continue;
    }
    auto id = name.substr(0, name.size() - 6);
    if (!valid_session_id(id)) {
      continue;
    }
    try {
      auto s = load_session(dir, id);
      if (!workspace.empty() && s.workspace != workspace) {
        continue;
      }
      if (!session_matches(s, query)) {
        continue;
      }
      SessionInfo info;
      info.id = id;
      info.name = s.name;
      info.workspace = s.workspace;
      info.mtime = entry.last_write_time();
      info.deleted = deleted;
      for (const auto& event : s.events) {
        auto text = first_user_text(event);
        if (!text.empty()) {
          info.preview = clip_line(text);
          break;
        }
      }
      all.push_back(std::move(info));
    } catch (...) {
    }
  }
  std::sort(all.begin(), all.end(),
            [](const SessionInfo& a, const SessionInfo& b) { return a.mtime > b.mtime; });
  if (static_cast<int>(all.size()) > limit) {
    all.resize(static_cast<size_t>(limit));
  }
  return all;
}

std::string html_escape(std::string_view s) {
  std::ostringstream out;
  for (char c : s) {
    switch (c) {
    case '&':
      out << "&amp;";
      break;
    case '<':
      out << "&lt;";
      break;
    case '>':
      out << "&gt;";
      break;
    case '"':
      out << "&quot;";
      break;
    case '\'':
      out << "&#39;";
      break;
    default:
      out << c;
      break;
    }
  }
  return out.str();
}

std::string html_images(const json& images) {
  if (!images.is_array()) {
    return {};
  }
  std::ostringstream out;
  for (const auto& image : images) {
    if (!image.is_object() || image.value("type", "") != "image") {
      continue;
    }
    const auto mime = image.value("mime_type", "");
    if (mime != "image/png" && mime != "image/jpeg" && mime != "image/webp") {
      continue;
    }
    out << "<figure><img style=\"max-width:100%;max-height:40rem\" src=\"data:" << mime
        << ";base64," << html_escape(image.value("data", "")) << "\" alt=\""
        << html_escape(image.value("name", "image")) << "\"></figure>\n";
  }
  return out.str();
}

std::string render_inline_markdown(std::string_view s) {
  std::ostringstream out;
  size_t i = 0;
  while (i < s.size()) {
    if (s[i] == '[') {
      auto close_bracket = s.find(']', i + 1);
      if (close_bracket != std::string_view::npos && close_bracket + 1 < s.size() &&
          s[close_bracket + 1] == '(') {
        auto close_paren = s.find(')', close_bracket + 2);
        if (close_paren != std::string_view::npos) {
          out << "<a href=\""
              << html_escape(s.substr(close_bracket + 2, close_paren - close_bracket - 2)) << "\">"
              << render_inline_markdown(s.substr(i + 1, close_bracket - i - 1)) << "</a>";
          i = close_paren + 1;
          continue;
        }
      }
    }
    if (s[i] == '`') {
      auto close = s.find('`', i + 1);
      if (close != std::string_view::npos) {
        out << "<code>" << html_escape(s.substr(i + 1, close - i - 1)) << "</code>";
        i = close + 1;
        continue;
      }
    }
    if (i + 1 < s.size() && s[i] == '*' && s[i + 1] == '*') {
      auto close = s.find("**", i + 2);
      if (close != std::string_view::npos) {
        out << "<strong>" << render_inline_markdown(s.substr(i + 2, close - i - 2)) << "</strong>";
        i = close + 2;
        continue;
      }
    }
    if (s[i] == '*') {
      auto close = s.find('*', i + 1);
      if (close != std::string_view::npos) {
        out << "<em>" << render_inline_markdown(s.substr(i + 1, close - i - 1)) << "</em>";
        i = close + 1;
        continue;
      }
    }
    size_t next = i + 1;
    while (next < s.size() && s[next] != '[' && s[next] != '`' && s[next] != '*') {
      ++next;
    }
    out << html_escape(s.substr(i, next - i));
    i = next;
  }
  return out.str();
}

std::string markdown_to_html(std::string_view md) {
  std::vector<std::string> lines;
  {
    std::istringstream in{std::string(md)};
    std::string line;
    while (std::getline(in, line)) {
      if (!line.empty() && line.back() == '\r') {
        line.pop_back();
      }
      lines.push_back(std::move(line));
    }
  }
  std::ostringstream out;
  size_t line = 0;
  while (line < lines.size()) {
    const auto& current = lines[line];
    if (current.rfind("```", 0) == 0) {
      ++line;
      std::ostringstream code;
      while (line < lines.size() && lines[line].rfind("```", 0) != 0) {
        code << lines[line] << '\n';
        ++line;
      }
      if (line < lines.size()) {
        ++line;
      }
      out << "<pre><code>" << html_escape(code.str()) << "</code></pre>\n";
      continue;
    }
    if (current.empty()) {
      ++line;
      continue;
    }
    if (!current.empty() && current[0] == '#') {
      int level = 0;
      while (level < static_cast<int>(current.size()) &&
             current[static_cast<size_t>(level)] == '#') {
        ++level;
      }
      if (level >= 1 && level <= 6 && static_cast<size_t>(level) < current.size() &&
          current[static_cast<size_t>(level)] == ' ') {
        out << "<h" << level << ">"
            << render_inline_markdown(current.substr(static_cast<size_t>(level) + 1)) << "</h"
            << level << ">\n";
        ++line;
        continue;
      }
    }
    std::ostringstream para;
    while (line < lines.size()) {
      const auto& part = lines[line];
      if (part.empty() || part.rfind("```", 0) == 0 ||
          (!part.empty() && part[0] == '#' && part.find(' ') != std::string::npos)) {
        break;
      }
      if (!para.str().empty()) {
        para << ' ';
      }
      para << part;
      ++line;
    }
    out << "<p>" << render_inline_markdown(para.str()) << "</p>\n";
  }
  return out.str();
}

constexpr const char* kHtmlStyles = R"(
:root{color-scheme:light dark;--bg:#fafafa;--fg:#1a1a1a;--muted:#666;--accent:#0066cc;--user-bg:#e8f4fc;--assistant-bg:#f0f0f0;--code:#0a7a0a;--error:#c00;--meta:#8040a0;--border:#ddd;--compaction-bg:#f5f0e8}
@media(prefers-color-scheme:dark){:root{--bg:#1a1a1a;--fg:#e8e8e8;--muted:#999;--accent:#5bc0de;--user-bg:#1e3a4a;--assistant-bg:#2a2a2a;--code:#7cfc7c;--error:#f66;--meta:#c9a0dc;--border:#444;--compaction-bg:#2a2520}}
*{box-sizing:border-box}
body{font-family:system-ui,-apple-system,sans-serif;line-height:1.5;margin:0;padding:1.5rem;background:var(--bg);color:var(--fg)}
header{border-bottom:1px solid var(--border);margin-bottom:1.5rem;padding-bottom:1rem}
header h1{margin:0 0 .5rem;font-size:1.5rem;color:var(--accent)}
header dl{margin:0;display:grid;grid-template-columns:auto 1fr;gap:.25rem 1rem;font-size:.9rem}
header dt{color:var(--muted);font-weight:600}
header dd{margin:0}
main{display:flex;flex-direction:column;gap:1rem}
.message{border:1px solid var(--border);border-radius:.5rem;padding:1rem}
.message .label{font-size:.75rem;font-weight:700;text-transform:uppercase;letter-spacing:.05em;margin-bottom:.5rem}
.message.user{background:var(--user-bg)}
.message.user .label{color:var(--accent)}
.message.assistant{background:var(--assistant-bg)}
.message.assistant .label{color:var(--muted)}
.message p:first-child{margin-top:0}
.message p:last-child{margin-bottom:0}
.message h1,.message h2,.message h3{margin:.75rem 0 .25rem;font-size:1rem}
.message pre{background:rgba(0,0,0,.06);border-radius:.25rem;padding:.75rem;overflow-x:auto;margin:.5rem 0}
@media(prefers-color-scheme:dark){.message pre{background:rgba(255,255,255,.06)}}
.message code{font-family:ui-monospace,monospace;font-size:.9em;color:var(--code)}
.tool-call{border:1px solid var(--border);border-radius:.5rem;padding:.5rem 1rem}
.tool-call summary{cursor:pointer;font-weight:600;color:var(--meta)}
.tool-call pre{margin:.5rem 0 0;font-size:.85rem;overflow-x:auto}
.tool-result{margin:0;padding:1rem;border-radius:.5rem;background:rgba(0,0,0,.04);overflow-x:auto;font-family:ui-monospace,monospace;font-size:.85rem;white-space:pre-wrap;word-break:break-word}
@media(prefers-color-scheme:dark){.tool-result{background:rgba(255,255,255,.04)}}
.tool-result.error{border:1px solid var(--error);color:var(--error)}
.compaction{border-left:3px solid var(--muted);background:var(--compaction-bg);padding:.75rem 1rem;border-radius:0 .5rem .5rem 0;font-size:.9rem;color:var(--muted)}
.compaction strong{display:block;margin-bottom:.25rem;color:var(--fg)}
)";

std::string export_html(const Session& session) {
  auto title = session.name.empty() ? "Session " + session.id : session.name;
  std::ostringstream out;
  out << "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n"
      << "<meta charset=\"utf-8\">\n"
      << "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n"
      << "<title>" << html_escape(title) << "</title>\n"
      << "<style>" << kHtmlStyles << "</style>\n"
      << "</head>\n<body>\n<header>\n<h1>" << html_escape(title) << "</h1>\n<dl>\n"
      << "<dt>id</dt><dd>" << html_escape(session.id) << "</dd>\n";
  if (!session.workspace.empty()) {
    out << "<dt>workspace</dt><dd>" << html_escape(session.workspace) << "</dd>\n";
  }
  if (!session.parent.empty()) {
    out << "<dt>forked from</dt><dd>" << html_escape(session.parent) << "</dd>\n";
  }
  out << "</dl>\n</header>\n<main>\n";
  for (const auto& event : session.events) {
    if (!event.is_object()) {
      continue;
    }
    auto type = event.value("type", "");
    if (type == "user") {
      out << "<article class=\"message user\"><div class=\"label\">You</div>\n"
          << "<div class=\"content\">" << html_escape(event_text(event)) << "</div>"
          << html_images(event.value("content", json::array())) << "</article>\n";
    } else if (type == "assistant") {
      auto text = event_text(event);
      if (!text.empty()) {
        out << "<article class=\"message assistant\"><div class=\"label\">Niminal</div>\n"
            << "<div class=\"content\">" << markdown_to_html(text) << "</div></article>\n";
      }
      if (event.contains("content") && event["content"].is_array()) {
        for (const auto& part : event["content"]) {
          if (!part.is_object() || part.value("type", "") != "tool_use") {
            continue;
          }
          auto name = part.value("name", "");
          auto input = part.value("input", json::object()).dump(2);
          out << "<details class=\"tool-call\"><summary>Tool: " << html_escape(name)
              << "</summary>\n<pre>" << html_escape(input) << "</pre></details>\n";
        }
      }
    } else if (type == "bash") {
      out << "<pre class=\"tool-result\">$ " << html_escape(event.value("command", "")) << "\n\n"
          << html_escape(event.value("output", "")) << "</pre>\n";
    } else if (type == "tool_result") {
      auto error = event.value("is_error", false);
      out << "<pre class=\"tool-result" << (error ? " error" : "") << "\">"
          << html_escape(event.value("output", "")) << "</pre>\n";
      out << html_images(event.value("images", json::array()));
    } else if (type == "compaction") {
      out << "<aside class=\"compaction\"><strong>Compaction</strong>\n"
          << html_escape(event.value("summary", "")) << "</aside>\n";
    }
  }
  out << "</main>\n</body>\n</html>\n";
  return out.str();
}

} // namespace

bool valid_session_id(std::string_view id) {
  if (id.empty()) {
    return false;
  }
  for (char c : id) {
    if ((c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '-' ||
        c == '_') {
      continue;
    }
    return false;
  }
  return true;
}

std::filesystem::path default_session_dir() {
  const char* home = std::getenv("HOME");
  if ((home == nullptr) || ((*home) == 0)) {
    throw std::runtime_error("HOME is not set; cannot use ~/.niminal/sessions");
  }
  return fs::path(home) / ".niminal" / "sessions";
}

std::string new_session_id() {
  auto us = std::chrono::duration_cast<std::chrono::microseconds>(
                std::chrono::system_clock::now().time_since_epoch())
                .count();
  return std::to_string(us);
}

void Session::append(const json& event) {
  events.push_back(event);
  if (!persist || path.empty()) {
    return;
  }
  fs::create_directories(fs::path(path).parent_path());
  if (!damaged_.empty()) {
    std::ifstream in(path);
    std::ostringstream raw;
    raw << in.rdbuf();
    if (raw.str() != damaged_) {
      throw std::runtime_error("Session changed on disk; reload before recovery.");
    }
    auto backup = path + ".recovery-" + new_session_id();
    {
      std::ofstream out(backup, std::ios::binary | std::ios::trunc);
      out << damaged_;
    }
    {
      std::ofstream out(path, std::ios::binary | std::ios::trunc);
      out << valid_prefix_;
    }
    needs_newline_ = !valid_prefix_.empty() && valid_prefix_.back() != '\n';
    damaged_.clear();
    valid_prefix_.clear();
  }
  std::ofstream out(path, std::ios::binary | std::ios::app);
  if (!out) {
    throw std::runtime_error("cannot write session " + path);
  }
  if (needs_newline_) {
    out << '\n';
  }
  bool empty = !fs::exists(path) || fs::file_size(path) == 0;
  if (empty && !workspace.empty()) {
    json header = {{"type", "session"}, {"workspace", workspace}};
    if (!parent.empty()) {
      header["parent"] = parent;
    }
    out << header.dump() << '\n';
  }
  out << event.dump() << '\n';
  out.flush();
  out.close();
  fsync_file(path);
  needs_newline_ = false;
}

void Session::add_user(const niminal::UserInput& input) {
  json content = json::array();
  if (!input.text.empty()) {
    content.push_back(text_block(input.text));
  }
  for (const auto& image : input.images) {
    content.push_back(image);
  }
  append(json{{"type", "user"}, {"role", "user"}, {"content", std::move(content)}});
}

void Session::add_bash(const std::string& command, const std::string& output,
                       bool exclude_from_context) {
  append(json{{"type", "bash"},
              {"command", command},
              {"output", output},
              {"exclude_from_context", exclude_from_context}});
}

void Session::add_assistant(const std::string& text, const json& tool_calls,
                            const std::string& model, const niminal::Usage& usage,
                            const std::string& reasoning_content, const json& reasoning_details) {
  json content = json::array();
  if (!text.empty()) {
    content.push_back(text_block(text));
  }
  if (tool_calls.is_array()) {
    for (const auto& call : tool_calls) {
      json input = json::object();
      try {
        auto args = call.value("function", json::object()).value("arguments", "");
        if (!args.empty()) {
          input = json::parse(args);
        }
      } catch (...) {
      }
      content.push_back({
          {"type", "tool_use"},
          {"id", call.value("id", "")},
          {"name", call.value("function", json::object()).value("name", "")},
          {"input", input},
      });
    }
  }
  json event = {{"type", "assistant"}, {"role", "assistant"}, {"content", content}};
  if (!reasoning_content.empty()) {
    event["reasoning_content"] = reasoning_content;
  }
  if (!reasoning_details.empty()) {
    event["reasoning_details"] = reasoning_details;
  }
  if (!model.empty()) {
    event["model"] = model;
  }
  if ((usage.input_tokens != 0) || (usage.output_tokens != 0) || usage.cache_reported) {
    event["prompt_tokens"] = usage.input_tokens;
    event["completion_tokens"] = usage.output_tokens;
    event["cache_read_tokens"] = usage.cache_read_tokens;
    event["cache_write_tokens"] = usage.cache_write_tokens;
    event["cache_reported"] = usage.cache_reported;
  }
  append(event);
}

niminal::Usage Session::usage_totals() const {
  niminal::Usage total;
  for (const auto& event : events) {
    if (!event.is_object() || event.value("type", "") != "assistant") {
      continue;
    }
    niminal::Usage u;
    u.input_tokens = event.value("prompt_tokens", 0);
    u.output_tokens = event.value("completion_tokens", 0);
    u.cache_read_tokens = event.value("cache_read_tokens", 0);
    u.cache_write_tokens = event.value("cache_write_tokens", 0);
    u.cache_reported =
        event.value("cache_reported", false) || u.cache_read_tokens > 0 || u.cache_write_tokens > 0;
    niminal::add_usage(total, u);
  }
  return total;
}

void Session::add_tool_result(const std::string& tool_id, const niminal::ToolResult& output,
                              bool is_error) {
  append(json{{"type", "tool_result"},
              {"id", tool_id},
              {"output", output.text},
              {"images", output.images},
              {"is_error", is_error}});
}

void Session::add_name(const std::string& title) {
  name = title;
  append(json{{"type", "name"}, {"name", title}});
}

void Session::add_selection(const std::string& model, const std::string& provider) {
  json event = {{"type", "selection"}, {"model", model}};
  if (!provider.empty()) {
    event["provider"] = provider;
  }
  append(std::move(event));
}

void Session::add_extension(const std::string& extension, const json& data) {
  append(json{{"type", "extension"}, {"extension", extension}, {"data", data}});
}

void Session::add_extension_message(const json& message) {
  append(json{{"type", "extension_message"}, {"message", message}});
}

void Session::add_compaction(const std::string& summary, int first_kept_index, int tokens_before,
                             const json& details) {
  json event = json::object();
  event["type"] = "compaction";
  event["summary"] = summary;
  event["first_kept_index"] = first_kept_index;
  event["tokens_before"] = tokens_before;
  if (!details.is_null() && !details.empty()) {
    event["details"] = details;
  }
  append(event);
}

int Session::latest_compaction_index() const {
  for (int i = static_cast<int>(events.size()) - 1; i >= 0; --i) {
    if (events[static_cast<size_t>(i)].value("type", "") == "compaction") {
      return i;
    }
  }
  return -1;
}

int Session::end_after_user_turn(int turn) const {
  if (turn < 1) {
    return -1;
  }
  int seen = 0;
  bool found = false;
  for (size_t i = 0; i < events.size(); ++i) {
    if (events[i].value("type", "") != "user") {
      continue;
    }
    ++seen;
    if (seen == turn) {
      found = true;
    } else if (seen == turn + 1) {
      return static_cast<int>(i);
    }
  }
  if (!found) {
    return -1;
  }
  return static_cast<int>(events.size());
}

std::vector<std::pair<int, std::string>> Session::user_turn_previews() const {
  std::vector<std::pair<int, std::string>> out;
  int turn = 0;
  for (const auto& event : events) {
    if (event.value("type", "") != "user") {
      continue;
    }
    ++turn;
    out.push_back({turn, clip_line(first_user_text(event))});
  }
  return out;
}

Session Session::fork(const fs::path& dir, int upto) const {
  Session copy;
  copy.id = new_session_id();
  copy.workspace = workspace;
  copy.parent = id;
  copy.name = name;
  copy.persist = persist;
  if (copy.persist) {
    copy.path = (dir / (copy.id + ".jsonl")).string();
  }
  size_t end = upto < 0 || static_cast<size_t>(upto) > events.size() ? events.size()
                                                                     : static_cast<size_t>(upto);
  for (size_t i = 0; i < end; ++i) {
    copy.append(events[i]);
  }
  return copy;
}

std::string Session::export_text(std::string_view format) const {
  if (format == "json") {
    json out = {{"id", id}, {"workspace", workspace}, {"name", name}};
    if (!parent.empty()) {
      out["parent"] = parent;
    }
    out["events"] = events;
    return out.dump(2) + "\n";
  }
  if (format == "html") {
    return export_html(*this);
  }
  std::ostringstream out;
  out << "# " << (name.empty() ? "Session " + id : name) << "\n\n";
  out << "- id: " << id << '\n';
  if (!workspace.empty()) {
    out << "- workspace: " << workspace << '\n';
  }
  if (!parent.empty()) {
    out << "- forked from: " << parent << '\n';
  }
  out << '\n';
  for (const auto& event : events) {
    if (!event.is_object()) {
      continue;
    }
    auto type = event.value("type", "");
    if (type == "user") {
      out << "## User\n\n" << event_text(event) << "\n\n";
    } else if (type == "assistant") {
      auto text = event_text(event);
      if (!text.empty()) {
        out << "## Assistant\n\n" << text << "\n\n";
      }
      if (event.contains("content") && event["content"].is_array()) {
        for (const auto& part : event["content"]) {
          if (!part.is_object() || part.value("type", "") != "tool_use") {
            continue;
          }
          out << "> **tool call** `" << part.value("name", "") << "` "
              << part.value("input", json::object()).dump() << "\n\n";
        }
      }
    } else if (type == "bash") {
      out << "## Shell\n\n$ " << event.value("command", "") << "\n\n```\n"
          << event.value("output", "") << "\n```\n\n";
    } else if (type == "tool_result") {
      out << "### Tool result" << (event.value("is_error", false) ? " (error)" : "") << "\n\n```\n"
          << event.value("output", "") << "\n```\n\n";
    } else if (type == "compaction") {
      out << "## Compaction\n\n" << event.value("summary", "") << "\n\n";
    }
  }
  return out.str();
}

int Session::recover_interrupted_tools() {
  std::vector<std::string> pending;
  for (const auto& event : events) {
    auto type = event.value("type", "");
    if (type == "assistant") {
      pending.clear();
      if (event.contains("content") && event["content"].is_array()) {
        for (const auto& part : event["content"]) {
          if (part.value("type", "") == "tool_use") {
            pending.push_back(part.value("id", ""));
          }
        }
      }
    } else if (type == "tool_result") {
      auto tool_id = event.value("id", "");
      pending.erase(std::remove(pending.begin(), pending.end(), tool_id), pending.end());
    }
  }
  const char* msg = "Interrupted before a tool result was saved. Execution outcome is unknown; "
                    "inspect current state before retrying any action.";
  int n = 0;
  for (const auto& tool_id : pending) {
    add_tool_result(tool_id, msg, true);
    ++n;
  }
  return n;
}

json Session::openai_messages() const {
  json out = json::array();
  size_t start = 0;
  int compact = latest_compaction_index();
  if (compact >= 0) {
    const auto& event = events[static_cast<size_t>(compact)];
    auto summary = event.value("summary", "");
    int kept = event.value("first_kept_index", 0);
    kept = std::max(kept, 0);
    start = static_cast<size_t>(kept);
    if (!summary.empty()) {
      json msg = json::object();
      msg["role"] = "user";
      msg["content"] = "The conversation history before this point was compacted into the "
                       "following summary:\n<summary>\n" +
                       summary + "\n</summary>";
      out.push_back(std::move(msg));
    }
  }
  start = std::min(start, events.size());
  for (size_t i = start; i < events.size(); ++i) {
    const auto& event = events[i];
    auto type = event.value("type", "");
    if (type == "user") {
      json content = event.value("content", json::array());
      bool has_image = false;
      std::string text;
      for (const auto& part : content) {
        if (part.value("type", "") == "image") {
          has_image = true;
        } else if (part.value("type", "") == "text") {
          text += part.value("text", "");
        }
      }
      out.push_back({{"role", "user"}, {"content", has_image ? content : json(text)}});
    } else if (type == "extension_message") {
      out.push_back(event.at("message"));
    } else if (type == "assistant") {
      json msg = {{"role", "assistant"}, {"content", ""}};
      json calls = json::array();
      if (event.contains("content") && event["content"].is_array()) {
        std::string text;
        for (const auto& part : event["content"]) {
          auto ptype = part.value("type", "");
          if (ptype == "text") {
            text += part.value("text", "");
          }
          if (ptype == "tool_use") {
            calls.push_back({
                {"id", part.value("id", "")},
                {"type", "function"},
                {"function",
                 {{"name", part.value("name", "")},
                  {"arguments", part.value("input", json::object()).dump()}}},
            });
          }
        }
        msg["content"] = text;
      }
      if (!calls.empty()) {
        msg["tool_calls"] = calls;
      }
      for (const auto* key : {"reasoning_content", "reasoning_details"}) {
        if (event.contains(key)) {
          msg[key] = event[key];
        }
      }
      out.push_back(std::move(msg));
    } else if (type == "bash") {
      if (!event.value("exclude_from_context", false)) {
        out.push_back({{"role", "user"},
                       {"content",
                        bash_user_message(event.value("command", ""), event.value("output", ""))}});
      }
    } else if (type == "tool_result") {
      out.push_back({{"role", "tool"},
                     {"tool_call_id", event.value("id", "")},
                     {"content", event.value("output", "")},
                     {"images", event.value("images", json::array())}});
    }
  }
  return out;
}

std::string Session::last_model() const {
  for (int i = static_cast<int>(events.size()) - 1; i >= 0; --i) {
    const auto& event = events[static_cast<size_t>(i)];
    auto type = event.value("type", "");
    if (type == "selection") {
      auto model = event.value("model", "");
      if (!model.empty()) {
        return model;
      }
    }
    if (type == "assistant") {
      auto model = event.value("model", "");
      if (!model.empty()) {
        return model;
      }
    }
  }
  return {};
}

std::string Session::last_provider() const {
  for (int i = static_cast<int>(events.size()) - 1; i >= 0; --i) {
    const auto& event = events[static_cast<size_t>(i)];
    if (event.value("type", "") != "selection") {
      continue;
    }
    auto provider = event.value("provider", "");
    if (!provider.empty()) {
      return provider;
    }
  }
  return {};
}

std::string Session::last_assistant_text() const {
  for (int i = static_cast<int>(events.size()) - 1; i >= 0; --i) {
    const auto& event = events[static_cast<size_t>(i)];
    if (event.value("type", "") != "assistant") {
      continue;
    }
    std::string text;
    if (event.contains("content") && event["content"].is_array()) {
      for (const auto& part : event["content"]) {
        if (part.is_object() && part.value("type", "") == "text") {
          text += part.value("text", "");
        }
      }
    }
    if (!text.empty()) {
      return text;
    }
  }
  return {};
}

std::string Session::describe() const {
  std::ostringstream out;
  out << "Session: " << id << '\n';
  out << "Name: " << (name.empty() ? "(none)" : name) << '\n';
  out << "Events: " << events.size() << '\n';
  out << "File: " << (path.empty() ? "(memory)" : path) << '\n';
  out << "Workspace: " << workspace << '\n';
  if (!parent.empty()) {
    out << "Forked from: " << parent << '\n';
  }
  return out.str();
}

Session create_session(const fs::path& dir, const std::string& workspace) {
  Session s;
  s.id = new_session_id();
  s.workspace = workspace;
  s.path = (dir / (s.id + ".jsonl")).string();
  s.persist = true;
  return s;
}

Session load_session(const fs::path& dir, const std::string& id) {
  if (!valid_session_id(id)) {
    throw std::runtime_error("Invalid session ID.");
  }
  Session s;
  s.id = id;
  s.path = (dir / (id + ".jsonl")).string();
  s.persist = true;
  if (!fs::exists(s.path)) {
    throw std::runtime_error("Session not found: " + id);
  }
  std::ifstream in(s.path);
  std::ostringstream raw;
  raw << in.rdbuf();
  auto original = raw.str();
  s.needs_newline_ = !original.empty() && original.back() != '\n';
  std::string prefix;
  bool first = true;
  std::istringstream lines(original);
  std::string line;
  while (std::getline(lines, line)) {
    std::string kept = line;
    kept += '\n';
    if (line.find_first_not_of(" \t\r") == std::string::npos) {
      prefix += kept;
      continue;
    }
    try {
      auto node = json::parse(line);
      if (first && node.value("type", "") == "session") {
        s.workspace = node.value("workspace", "");
        s.parent = node.value("parent", "");
        first = false;
        prefix += kept;
        continue;
      }
      first = false;
      s.events.push_back(node);
      if (node.value("type", "") == "name") {
        s.name = node.value("name", "");
      }
      prefix += kept;
    } catch (...) {
      s.damaged_ = original;
      s.valid_prefix_ = prefix;
      break;
    }
  }
  return s;
}

std::vector<SessionInfo> list_sessions(const fs::path& dir, const std::string& workspace,
                                       int limit) {
  return collect_sessions(dir, workspace, limit, {}, false);
}

std::vector<SessionInfo> search_sessions(const fs::path& dir, const std::string& workspace,
                                         const std::string& query, int limit) {
  return collect_sessions(dir, workspace, limit, query, false);
}

std::vector<SessionInfo> list_deleted_sessions(const fs::path& dir) {
  return collect_sessions(trash_dir(dir), {}, 20, {}, true);
}

bool delete_session(const fs::path& dir, const std::string& id) {
  if (!valid_session_id(id)) {
    return false;
  }
  std::error_code ec;
  auto src = dir / (id + ".jsonl");
  if (!fs::exists(src, ec)) {
    return false;
  }
  fs::create_directories(trash_dir(dir), ec);
  fs::rename(src, trash_dir(dir) / (id + ".jsonl"), ec);
  return !ec;
}

bool restore_session(const fs::path& dir, const std::string& id) {
  if (!valid_session_id(id)) {
    return false;
  }
  std::error_code ec;
  auto src = trash_dir(dir) / (id + ".jsonl");
  if (!fs::exists(src, ec)) {
    return false;
  }
  auto dest = dir / (id + ".jsonl");
  if (fs::exists(dest, ec)) {
    return false;
  }
  fs::rename(src, dest, ec);
  return !ec;
}

std::string relative_age(fs::file_time_type mtime) {
  auto now_file = fs::file_time_type::clock::now();
  auto now_sys = std::chrono::system_clock::now();
  auto sys =
      now_sys - std::chrono::duration_cast<std::chrono::system_clock::duration>(now_file - mtime);
  auto secs =
      std::chrono::duration_cast<std::chrono::seconds>(std::chrono::system_clock::now() - sys)
          .count();
  if (secs < 0) {
    secs = 0;
  }
  if (secs < 60) {
    return std::to_string(secs) + "s ago";
  }
  if (secs < 3600) {
    return std::to_string(secs / 60) + "m ago";
  }
  if (secs < 86400) {
    return std::to_string(secs / 3600) + "h ago";
  }
  return std::to_string(secs / 86400) + "d ago";
}

std::string format_session_list(const std::vector<SessionInfo>& infos,
                                const std::string& current_id, std::string_view heading) {
  if (infos.empty()) {
    return "No sessions.";
  }
  std::ostringstream out;
  out << heading << ":\n";
  for (const auto& info : infos) {
    auto label =
        !info.name.empty() ? info.name : (!info.preview.empty() ? info.preview : "(empty)");
    out << "  " << relative_age(info.mtime) << "   " << label << "  #" << info.id;
    if (info.id == current_id) {
      out << "  (current)";
    }
    out << '\n';
  }
  return out.str();
}

void bind_session(niminal::Agent& agent, Session& session) {
  agent.persist_user = [&session](const niminal::UserInput& input) { session.add_user(input); };
  agent.persist_extension_message = [&session](const nlohmann::json& message) {
    session.add_extension_message(message);
  };
  agent.persist_assistant =
      [&session](const std::string& text, const std::vector<niminal::ToolCall>& calls,
                 const std::string& model, const niminal::Usage& usage,
                 const std::string& reasoning_content, const nlohmann::json& reasoning_details) {
        nlohmann::json arr = nlohmann::json::array();
        for (const auto& call : calls) {
          arr.push_back({{"id", call.id},
                         {"type", "function"},
                         {"function", {{"name", call.name}, {"arguments", call.arguments}}}});
        }
        session.add_assistant(text, arr, model, usage, reasoning_content, reasoning_details);
      };
  agent.persist_tool = [&session](const std::string& id, const niminal::ToolResult& output,
                                  bool error) { session.add_tool_result(id, output, error); };
  agent.conversation_id = session.id;
}

} // namespace niminal::app
