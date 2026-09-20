#include "session.hpp"

#include <niminal/agent.hpp>

#include <algorithm>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <sstream>
#include <stdexcept>
#include <unistd.h>

namespace niminal::app {
namespace fs = std::filesystem;
using json = nlohmann::json;

namespace {

void fsync_file(const fs::path& path) {
  FILE* f = std::fopen(path.c_str(), "ab");
  if (!f) return;
  std::fflush(f);
  fsync(fileno(f));
  std::fclose(f);
}

std::string clip_line(std::string s, size_t n = 60) {
  for (char& c : s)
    if (c == '\n' || c == '\r' || c == '\t') c = ' ';
  if (s.size() > n) {
    s.resize(n);
    s += "…";
  }
  return s;
}

std::string first_user_text(const json& event) {
  if (event.value("type", "") != "user") return {};
  if (!event.contains("content") || !event["content"].is_array()) return {};
  for (const auto& part : event["content"]) {
    if (part.value("type", "") == "text") return part.value("text", "");
  }
  return {};
}

json text_block(const std::string& text) {
  return json{{"type", "text"}, {"text", text}};
}

}  // namespace

bool valid_session_id(std::string_view id) {
  if (id.empty()) return false;
  for (char c : id) {
    if ((c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') ||
        (c >= 'A' && c <= 'Z') || c == '-' || c == '_')
      continue;
    return false;
  }
  return true;
}

std::filesystem::path default_session_dir() {
  const char* home = std::getenv("HOME");
  if (!home || !*home)
    throw std::runtime_error("HOME is not set; cannot use ~/.niminal/sessions");
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
  if (!persist || path.empty()) return;
  fs::create_directories(fs::path(path).parent_path());
  if (!damaged_.empty()) {
    std::ifstream in(path);
    std::ostringstream raw;
    raw << in.rdbuf();
    if (raw.str() != damaged_)
      throw std::runtime_error("Session changed on disk; reload before recovery.");
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
  if (!out) throw std::runtime_error("cannot write session " + path);
  if (needs_newline_) out << '\n';
  bool empty = !fs::exists(path) || fs::file_size(path) == 0;
  if (empty && !workspace.empty())
    out << json{{"type", "session"}, {"workspace", workspace}}.dump() << '\n';
  out << event.dump() << '\n';
  out.flush();
  out.close();
  fsync_file(path);
  needs_newline_ = false;
}

void Session::add_user(const std::string& text) {
  append(json{{"type", "user"},
              {"role", "user"},
              {"content", json::array({text_block(text)})}});
}

void Session::add_assistant(const std::string& text, const json& tool_calls,
                            const std::string& model, const niminal::Usage& usage) {
  json content = json::array();
  if (!text.empty()) content.push_back(text_block(text));
  if (tool_calls.is_array()) {
    for (const auto& call : tool_calls) {
      json input = json::object();
      try {
        auto args = call.value("function", json::object()).value("arguments", "");
        if (!args.empty()) input = json::parse(args);
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
  if (!model.empty()) event["model"] = model;
  if (usage.input_tokens || usage.output_tokens || usage.cache_reported) {
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
    if (!event.is_object() || event.value("type", "") != "assistant") continue;
    niminal::Usage u;
    u.input_tokens = event.value("prompt_tokens", 0);
    u.output_tokens = event.value("completion_tokens", 0);
    u.cache_read_tokens = event.value("cache_read_tokens", 0);
    u.cache_write_tokens = event.value("cache_write_tokens", 0);
    u.cache_reported = event.value("cache_reported", false) ||
                       u.cache_read_tokens > 0 || u.cache_write_tokens > 0;
    niminal::add_usage(total, u);
  }
  return total;
}

void Session::add_tool_result(const std::string& id, const std::string& output,
                              bool is_error) {
  append(json{{"type", "tool_result"},
              {"id", id},
              {"output", output},
              {"is_error", is_error}});
}

void Session::add_name(const std::string& title) {
  name = title;
  append(json{{"type", "name"}, {"name", title}});
}

void Session::add_selection(const std::string& model, const std::string& provider) {
  json event = {{"type", "selection"}, {"model", model}};
  if (!provider.empty()) event["provider"] = provider;
  append(std::move(event));
}

void Session::add_extension(const std::string& extension, const json& data) {
  append(json{{"type", "extension"}, {"extension", extension}, {"data", data}});
}

void Session::add_compaction(const std::string& summary, int first_kept_index,
                             int tokens_before, const json& details) {
  json event = json::object();
  event["type"] = "compaction";
  event["summary"] = summary;
  event["first_kept_index"] = first_kept_index;
  event["tokens_before"] = tokens_before;
  if (!details.is_null() && !details.empty()) event["details"] = details;
  append(event);
}

int Session::latest_compaction_index() const {
  for (int i = static_cast<int>(events.size()) - 1; i >= 0; --i) {
    if (events[static_cast<size_t>(i)].value("type", "") == "compaction")
      return i;
  }
  return -1;
}

int Session::recover_interrupted_tools() {
  std::vector<std::string> pending;
  for (const auto& event : events) {
    auto type = event.value("type", "");
    if (type == "assistant") {
      pending.clear();
      if (event.contains("content") && event["content"].is_array()) {
        for (const auto& part : event["content"]) {
          if (part.value("type", "") == "tool_use")
            pending.push_back(part.value("id", ""));
        }
      }
    } else if (type == "tool_result") {
      auto id = event.value("id", "");
      pending.erase(std::remove(pending.begin(), pending.end(), id),
                    pending.end());
    }
  }
  const char* msg =
      "Interrupted before a tool result was saved. Execution outcome is unknown; "
      "inspect current state before retrying any action.";
  int n = 0;
  for (const auto& id : pending) {
    add_tool_result(id, msg, true);
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
    if (kept < 0) kept = 0;
    start = static_cast<size_t>(kept);
    if (!summary.empty()) {
      json msg = json::object();
      msg["role"] = "user";
      msg["content"] =
          "The conversation history before this point was compacted into the "
          "following summary:\n<summary>\n" +
          summary + "\n</summary>";
      out.push_back(std::move(msg));
    }
  }
  if (start > events.size()) start = events.size();
  for (size_t i = start; i < events.size(); ++i) {
    const auto& event = events[i];
    auto type = event.value("type", "");
    if (type == "user") {
      std::string text;
      if (event.contains("content") && event["content"].is_array()) {
        for (const auto& part : event["content"])
          if (part.value("type", "") == "text") text += part.value("text", "");
      }
      out.push_back({{"role", "user"}, {"content", text}});
    } else if (type == "assistant") {
      json msg = {{"role", "assistant"}, {"content", ""}};
      json calls = json::array();
      if (event.contains("content") && event["content"].is_array()) {
        std::string text;
        for (const auto& part : event["content"]) {
          auto ptype = part.value("type", "");
          if (ptype == "text") text += part.value("text", "");
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
      if (!calls.empty()) msg["tool_calls"] = calls;
      out.push_back(std::move(msg));
    } else if (type == "tool_result") {
      out.push_back({{"role", "tool"},
                     {"tool_call_id", event.value("id", "")},
                     {"content", event.value("output", "")}});
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
      if (!model.empty()) return model;
    }
    if (type == "assistant") {
      auto model = event.value("model", "");
      if (!model.empty()) return model;
    }
  }
  return {};
}

std::string Session::last_provider() const {
  for (int i = static_cast<int>(events.size()) - 1; i >= 0; --i) {
    const auto& event = events[static_cast<size_t>(i)];
    if (event.value("type", "") != "selection") continue;
    auto provider = event.value("provider", "");
    if (!provider.empty()) return provider;
  }
  return {};
}

std::string Session::last_assistant_text() const {
  for (int i = static_cast<int>(events.size()) - 1; i >= 0; --i) {
    const auto& event = events[static_cast<size_t>(i)];
    if (event.value("type", "") != "assistant") continue;
    std::string text;
    if (event.contains("content") && event["content"].is_array()) {
      for (const auto& part : event["content"]) {
        if (part.is_object() && part.value("type", "") == "text")
          text += part.value("text", "");
      }
    }
    if (!text.empty()) return text;
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
  if (!valid_session_id(id)) throw std::runtime_error("Invalid session ID.");
  Session s;
  s.id = id;
  s.path = (dir / (id + ".jsonl")).string();
  s.persist = true;
  if (!fs::exists(s.path)) throw std::runtime_error("Session not found: " + id);
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
        first = false;
        prefix += kept;
        continue;
      }
      first = false;
      s.events.push_back(node);
      if (node.value("type", "") == "name") s.name = node.value("name", "");
      prefix += kept;
    } catch (...) {
      s.damaged_ = original;
      s.valid_prefix_ = prefix;
      break;
    }
  }
  return s;
}

std::vector<SessionInfo> list_sessions(const fs::path& dir,
                                       const std::string& workspace, int limit) {
  std::vector<SessionInfo> all;
  std::error_code ec;
  if (!fs::exists(dir, ec)) return {};
  for (const auto& entry : fs::directory_iterator(dir, ec)) {
    if (!entry.is_regular_file()) continue;
    auto name = entry.path().filename().string();
    if (name.size() < 6 || !name.ends_with(".jsonl")) continue;
    auto id = name.substr(0, name.size() - 6);
    if (!valid_session_id(id)) continue;
    try {
      auto s = load_session(dir, id);
      if (!workspace.empty() && s.workspace != workspace) continue;
      SessionInfo info;
      info.id = id;
      info.name = s.name;
      info.workspace = s.workspace;
      info.mtime = entry.last_write_time();
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
  std::sort(all.begin(), all.end(), [](const SessionInfo& a, const SessionInfo& b) {
    return a.mtime > b.mtime;
  });
  if (static_cast<int>(all.size()) > limit) all.resize(static_cast<size_t>(limit));
  return all;
}

std::string relative_age(fs::file_time_type mtime) {
  auto now_file = fs::file_time_type::clock::now();
  auto now_sys = std::chrono::system_clock::now();
  auto sys = now_sys - std::chrono::duration_cast<std::chrono::system_clock::duration>(
                           now_file - mtime);
  auto secs = std::chrono::duration_cast<std::chrono::seconds>(
                  std::chrono::system_clock::now() - sys)
                  .count();
  if (secs < 0) secs = 0;
  if (secs < 60) return std::to_string(secs) + "s ago";
  if (secs < 3600) return std::to_string(secs / 60) + "m ago";
  if (secs < 86400) return std::to_string(secs / 3600) + "h ago";
  return std::to_string(secs / 86400) + "d ago";
}

std::string format_session_list(const std::vector<SessionInfo>& infos,
                                const std::string& current_id) {
  if (infos.empty()) return "No sessions for this workspace.";
  std::ostringstream out;
  out << "Sessions (newest first):\n";
  for (const auto& info : infos) {
    auto label = !info.name.empty() ? info.name
                                    : (!info.preview.empty() ? info.preview : "(empty)");
    out << "  " << relative_age(info.mtime) << "   " << label << "  #" << info.id;
    if (info.id == current_id) out << "  (current)";
    out << '\n';
  }
  return out.str();
}

void bind_session(niminal::Agent& agent, Session& session) {
  agent.persist_user = [&session](const std::string& text) {
    session.add_user(text);
  };
  agent.persist_assistant = [&session](const std::string& text,
                                       const std::vector<niminal::ToolCall>& calls,
                                       const std::string& model,
                                       const niminal::Usage& usage) {
    nlohmann::json arr = nlohmann::json::array();
    for (const auto& call : calls) {
      arr.push_back({{"id", call.id},
                     {"type", "function"},
                     {"function",
                      {{"name", call.name}, {"arguments", call.arguments}}}});
    }
    session.add_assistant(text, arr, model, usage);
  };
  agent.persist_tool = [&session](const std::string& id, const std::string& output,
                                  bool error) {
    session.add_tool_result(id, output, error);
  };
  agent.conversation_id = session.id;
}

}  // namespace niminal::app
