#include "slash.hpp"

#include "extensions.hpp"
#include "models_dev.hpp"
#include "prompts.hpp"
#include "session.hpp"
#include "skills.hpp"
#include "theme.hpp"
#include "thinking.hpp"

#include <niminal/providers.hpp>
#include <niminal/text.hpp>

#include <algorithm>
#include <string_view>
#include <utility>

namespace niminal::app {
namespace {

struct SlashSpec {
  const char* name;
  const char* usage;
  const char* hint;
};

constexpr SlashSpec kSlash[] = {
    {"/help", "/help", "this list"},
    {"/provider", "/provider [name]", "show or set the provider"},
    {"/model", "/model [ID]", "show or set the model"},
    {"/thinking", "/thinking [level]", "show or set reasoning"},
    {"/theme", "/theme [mode]", "show or set light|dark|auto"},
    {"/permissions", "/permissions [clear]", "show or clear tool grants"},
    {"/trust", "/trust [on|off]", "show or set project resource trust"},
    {"/yolo", "/yolo [off]", "auto-approve tools for this process"},
    {"/models", "/models refresh", "refresh the models.dev catalog"},
    {"/session", "/session", "show the current session"},
    {"/name", "/name [title]", "show or set the session name"},
    {"/resume", "/resume [ID]", "list or load a session"},
    {"/search", "/search TEXT", "search sessions for text"},
    {"/fork", "/fork [title]", "copy this session into a new one"},
    {"/export", "/export [PATH]", "write this session as Markdown or JSON"},
    {"/delete", "/delete ID", "move a session to the trash"},
    {"/restore", "/restore [ID]", "list or restore a deleted session"},
    {"/new", "/new", "start a new session"},
    {"/clear", "/clear", "same as /new"},
    {"/copy", "/copy", "copy the last error or reply"},
    {"/retry", "/retry", "retry the last failed request"},
    {"/compact", "/compact", "summarize older session history"},
    {"/reload", "/reload", "reload trusted project resources"},
    {"/skill:", "/skill:NAME [request]", "load a skill"},
    {"/quit", "/quit", "exit"},
    {"/exit", "/exit", "exit"},
};

bool contains_ci(std::string_view s, std::string_view p) {
  return niminal::lower_copy(std::string(s)).find(niminal::lower_copy(std::string(p))) !=
         std::string::npos;
}

std::string session_title(const SessionInfo& info) {
  return !info.name.empty() ? info.name : (!info.preview.empty() ? info.preview : "(empty)");
}

bool session_matches_info(const SessionInfo& info, const std::string& query) {
  return contains_ci(info.id, query) || contains_ci(info.name, query) ||
         contains_ci(info.preview, query);
}

void sort_suggestions(std::vector<Suggestion>& out) {
  std::sort(out.begin(), out.end(), [](const Suggestion& a, const Suggestion& b) {
    return niminal::lower_copy(a.fill) < niminal::lower_copy(b.fill);
  });
}

std::vector<Suggestion> suggest_models(const std::string& query, std::string_view provider,
                                       const std::vector<std::string>& recents) {
  constexpr int kMin = 2;
  constexpr int kCap = 50;
  std::vector<Suggestion> out;
  std::vector<std::string> used;
  auto add = [&](const std::string& id, int context) {
    if (id.empty()) {
      return;
    }
    auto key = niminal::lower_copy(id);
    for (const auto& x : used) {
      if (x == key) {
        return;
      }
    }
    used.push_back(key);
    auto label = id;
    auto ctx = format_context_k(context);
    if (!ctx.empty()) {
      label += "  " + ctx;
    }
    out.push_back({"/model " + id, std::move(label)});
  };
  auto q = niminal::lower_copy(query);
  if (static_cast<int>(q.size()) >= kMin) {
    auto catalog = search_catalog(provider, query, kCap);
    if (!catalog.empty()) {
      for (const auto& row : catalog) {
        add(row.id, row.context);
      }
      return out;
    }
  }
  for (const auto& id : recents) {
    if (!q.empty() && !contains_ci(id, q)) {
      continue;
    }
    add(id, 0);
  }
  if (static_cast<int>(q.size()) < kMin && !q.empty()) {
    return out;
  }
  int remain = kCap - static_cast<int>(out.size());
  if (remain <= 0) {
    return out;
  }
  for (const auto& row : search_catalog(provider, query, remain, recents)) {
    add(row.id, row.context);
  }
  return out;
}

} // namespace

std::string trim_copy(std::string s) {
  while (!s.empty() && (s.back() == ' ' || s.back() == '\n' || s.back() == '\r')) {
    s.pop_back();
  }
  size_t i = 0;
  while (i < s.size() && (s[i] == ' ' || s[i] == '\n' || s[i] == '\r')) {
    ++i;
  }
  return s.substr(i);
}

std::pair<std::string, std::string> split_slash(const std::string& prompt) {
  auto space = prompt.find(' ');
  auto cmd = space == std::string::npos ? prompt : prompt.substr(0, space);
  auto arg = space == std::string::npos ? std::string() : trim_copy(prompt.substr(space + 1));
  for (char& c : cmd) {
    if (c >= 'A' && c <= 'Z') {
      c = static_cast<char>(c - 'A' + 'a');
    }
  }
  return {cmd, arg};
}

bool is_builtin_slash(std::string_view command) {
  for (const auto& spec : kSlash) {
    if (command == spec.name) {
      return true;
    }
  }
  return false;
}

std::string slash_help() {
  size_t width = std::string_view("/NAME [text]").size();
  for (const auto& spec : kSlash) {
    width = std::max(width, std::string_view(spec.usage).size());
  }
  std::string out;
  auto line = [&](std::string_view usage, std::string_view hint) {
    out += usage;
    out.append(width - usage.size(), ' ');
    out += "  ";
    out += hint;
    out += '\n';
  };
  for (const auto& spec : kSlash) {
    if (std::string_view(spec.name) == "/exit") {
      continue;
    }
    line(spec.usage, spec.hint);
  }
  line("/NAME [text]", "expand a Markdown prompt template");
  out += R"(
Enter sends. While a turn runs, Enter queues a steering message.
Alt-J or Shift-Enter inserts a newline.
Type @ to add a workspace file. Gitignored files and dependency folders are hidden.
Tab accepts a suggestion. Up/Down picks one, or walks prompt history.
Page Up/Down and the trackpad scroll the transcript.
Click a thinking, tool, or diff card to expand it. Ctrl+O toggles the latest card.
Ctrl+Shift+O expands or collapses every card.
Esc interrupts a running turn, or clears the composer.
Drag to copy. Ctrl-V pastes into the composer. /copy copies the last reply.
Ctrl-G opens the composer in the configured editor, else $VISUAL or $EDITOR
(nano if none is set).
Ctrl-C quits.)";
  return out;
}

std::vector<Suggestion> slash_suggestions(const std::string& draft,
                                          const std::filesystem::path& dir,
                                          const std::string& workspace, std::string_view provider,
                                          std::string_view model,
                                          const std::vector<std::string>& recents,
                                          const std::vector<ExtensionCommand>& extension_commands) {
  if (draft.empty() || draft[0] != '/' || draft.find('\n') != std::string::npos) {
    return {};
  }
  auto [cmd, arg] = split_slash(draft);
  bool trailing = !draft.empty() && (draft.back() == ' ' || draft.back() == '\t');

  if (cmd.starts_with("/skill:")) {
    std::vector<Suggestion> out;
    auto query = cmd.substr(7);
    for (const auto& skill : discover_skills(workspace)) {
      if (!contains_ci(skill.name, query)) {
        continue;
      }
      out.push_back(
          {"/skill:" + skill.name + " ",
           "/skill:" + skill.name + (skill.description.empty() ? "" : "  " + skill.description)});
    }
    sort_suggestions(out);
    return out;
  }

  if (cmd == "/resume" && (trailing || !arg.empty())) {
    std::vector<Suggestion> out;
    try {
      for (const auto& info : list_sessions(dir, workspace)) {
        if (!arg.empty() && !session_matches_info(info, arg)) {
          continue;
        }
        out.push_back({"/resume " + info.id, info.id + "  " + session_title(info)});
        if (out.size() == 8) {
          break;
        }
      }
    } catch (...) {
    }
    if (!out.empty()) {
      return out;
    }
  }

  if (cmd == "/restore" && (trailing || !arg.empty())) {
    std::vector<Suggestion> out;
    try {
      for (const auto& info : list_deleted_sessions(dir)) {
        if (!arg.empty() && !session_matches_info(info, arg)) {
          continue;
        }
        out.push_back({"/restore " + info.id, info.id + "  " + session_title(info)});
        if (out.size() == 8) {
          break;
        }
      }
    } catch (...) {
    }
    if (!out.empty()) {
      return out;
    }
  }

  if (cmd == "/model" && (trailing || !arg.empty())) {
    auto models = suggest_models(arg, provider, recents);
    if (!models.empty()) {
      return models;
    }
  }

  if (cmd == "/thinking" && (trailing || !arg.empty())) {
    std::vector<Suggestion> out;
    for (const auto& level : thinking_choices(provider, model)) {
      if (!arg.empty() && !level.starts_with(arg)) {
        continue;
      }
      out.push_back({"/thinking " + level, level});
    }
    if (!out.empty()) {
      return out;
    }
  }

  if (cmd == "/theme" && (trailing || !arg.empty())) {
    std::vector<Suggestion> out;
    for (auto mode : {ThemeMode::automatic, ThemeMode::light, ThemeMode::dark}) {
      std::string name = theme_mode_name(mode);
      if (!arg.empty() && !name.starts_with(niminal::lower_copy(arg))) {
        continue;
      }
      out.push_back({"/theme " + name, name});
    }
    if (!out.empty()) {
      return out;
    }
  }

  if (cmd == "/models" && (trailing || !arg.empty())) {
    if (arg.empty() || std::string_view("refresh").starts_with(arg)) {
      return {{"/models refresh", "/models refresh  fetch models.dev"}};
    }
  }

  if (cmd == "/provider" && (trailing || !arg.empty())) {
    std::vector<Suggestion> out;
    for (const auto& spec : niminal::all_providers()) {
      if (!arg.empty() && !spec.name.starts_with(arg)) {
        continue;
      }
      out.push_back({"/provider " + std::string(spec.name),
                     std::string(spec.name) + "  " + std::string(spec.default_model)});
    }
    sort_suggestions(out);
    if (!out.empty()) {
      return out;
    }
  }

  if (!arg.empty()) {
    return {};
  }

  std::vector<Suggestion> out;
  for (const auto& spec : kSlash) {
    if (!std::string_view(spec.name).starts_with(cmd)) {
      continue;
    }
    std::string fill = spec.name;
    if (std::string(spec.usage) != spec.name && fill.back() != ':') {
      fill += ' ';
    }
    out.push_back({fill, std::string(spec.usage) + "  " + spec.hint});
  }
  for (const auto& prompt : discover_prompts(workspace)) {
    auto slash = "/" + prompt.name;
    if (is_builtin_slash(niminal::lower_copy(slash))) {
      continue;
    }
    if (!niminal::lower_copy(slash).starts_with(cmd)) {
      continue;
    }
    out.push_back({slash + " ", slash + (prompt.description.empty() ? std::string()
                                                                    : "  " + prompt.description)});
  }
  for (const auto& command : extension_commands) {
    auto slash = "/" + command.name;
    if (is_builtin_slash(niminal::lower_copy(slash))) {
      continue;
    }
    if (!niminal::lower_copy(slash).starts_with(cmd)) {
      continue;
    }
    out.push_back(
        {slash + " ",
         slash + (command.description.empty() ? std::string() : "  " + command.description)});
  }
  sort_suggestions(out);
  return out;
}

} // namespace niminal::app
