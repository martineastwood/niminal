#include "tui.hpp"
#include "compaction.hpp"
#include "config.hpp"
#include "diff.hpp"
#include "markdown.hpp"
#include "mentions.hpp"
#include "models_dev.hpp"
#include "permissions.hpp"
#include "provider.hpp"
#include "prompts.hpp"
#include "session.hpp"
#include "skills.hpp"
#include "theme.hpp"
#include "thinking.hpp"
#include "trust.hpp"

#include <niminal/openai.hpp>

#include <ftxui/component/component.hpp>
#include <ftxui/component/mouse.hpp>
#include <ftxui/component/screen_interactive.hpp>
#include <ftxui/dom/elements.hpp>
#include <ftxui/screen/string.hpp>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <exception>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <mutex>
#include <optional>
#include <sstream>
#include <string_view>
#include <thread>
#include <termios.h>
#include <unistd.h>
#include <unordered_map>
#include <utility>
#include <vector>

namespace niminal::app {
namespace {

using namespace ftxui;

enum class BlockKind {
  user,
  assistant,
  thinking,
  tool,
  diff,
  error,
  status,
  approval,
};

struct Block {
  BlockKind kind = BlockKind::assistant;
  std::string text;
  std::string path;
  bool created = false;

  Block() = default;
  Block(BlockKind kind, std::string text)
      : kind(kind), text(std::move(text)) {}
  Block(BlockKind kind, std::string text, std::string path, bool created)
      : kind(kind),
        text(std::move(text)),
        path(std::move(path)),
        created(created) {}
};

struct PendingFileChange {
  std::filesystem::path path;
  std::string relative;
  bool before_exists = false;
  std::string before;
};

std::optional<std::string> read_text_file(const std::filesystem::path& path) {
  std::error_code ec;
  if (!std::filesystem::is_regular_file(path, ec) ||
      std::filesystem::file_size(path, ec) > 200'000)
    return std::nullopt;
  std::ifstream in(path, std::ios::binary);
  if (!in) return std::nullopt;
  std::ostringstream out;
  out << in.rdbuf();
  auto text = out.str();
  if (text.find('\0') != std::string::npos) return std::nullopt;
  return text;
}

Element render_diff_card(const Block& block, const Theme& theme) {
  Elements lines;
  std::istringstream in(block.text);
  std::string line;
  bool skip_header = true;
  while (std::getline(in, line)) {
    if (skip_header) { skip_header = false; continue; }  // skip @@ marker
    Element row = text(line.empty() ? " " : line);
    if (!line.empty() && line.front() == '+')
      row = row | color(theme.add);
    else if (!line.empty() && line.front() == '-')
      row = row | color(theme.del);
    else if (!line.empty() && line.front() == '!')
      row = row | color(theme.meta);
    else
      row = row | dim;
    lines.push_back(std::move(row));
  }
  auto badge_color = block.created ? theme.add : theme.accent;
  auto badge = text(block.created ? "created" : "updated") | bold | color(badge_color);
  return vbox({hbox({badge, text("  " + block.path) | bold | dim}),
               separatorLight() | dim, vbox(std::move(lines))});
}

std::string clip_text(std::string text, size_t max_chars, int max_lines) {
  int lines = 1;
  for (char c : text)
    if (c == '\n') ++lines;
  if (text.size() > max_chars) {
    text.resize(max_chars);
    text += "\n[truncated]";
    return text;
  }
  if (lines <= max_lines) return text;
  std::string out;
  int kept = 0;
  for (char c : text) {
    out += c;
    if (c == '\n' && ++kept >= max_lines) break;
  }
  out += "[truncated]";
  return out;
}

std::string one_line(std::string s, size_t n) {
  for (char& c : s)
    if (c == '\n' || c == '\r' || c == '\t') c = ' ';
  if (s.size() > n) {
    s.resize(n);
    s += "…";
  }
  return s;
}

std::string tool_summary(const std::string& name, const std::string& args) {
  json j = json::object();
  try {
    if (!args.empty()) j = json::parse(args);
  } catch (...) {
    return "▸ " + name + "  " + one_line(args, 120);
  }
  if (!j.is_object())
    return "▸ " + name + (args.empty() ? "" : "  " + one_line(args, 120));
  std::string detail;
  if (name == "bash")
    detail = "$ " + (j.contains("command") && j["command"].is_string()
                         ? j["command"].get<std::string>()
                         : std::string());
  else if (name == "skill")
    detail = j.contains("name") && j["name"].is_string()
                 ? j["name"].get<std::string>()
                 : std::string();
  else if (j.contains("path") && j["path"].is_string()) {
    detail = j["path"].get<std::string>();
    if (j.contains("pattern") && j["pattern"].is_string())
      detail += "  " + j["pattern"].get<std::string>();
    else if (j.contains("old_text") && j["old_text"].is_string())
      detail += "  " + one_line(j["old_text"].get<std::string>(), 60);
  } else if (j.contains("pattern") && j["pattern"].is_string())
    detail = j["pattern"].get<std::string>();
  else if (!j.empty())
    detail = j.dump();
  if (detail.empty()) return "▸ " + name;
  return "▸ " + name + "  " + one_line(detail, 120);
}

Decorator block_style(BlockKind kind, const Theme& theme) {
  switch (kind) {
    case BlockKind::user:
      return color(theme.accent);
    case BlockKind::tool:
      return color(theme.meta);
    case BlockKind::thinking:
      return [](Element e) { return e | dim | italic; };
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

bool is_send(const Event& e) {
  if (e == Event::Return) return true;
  if (e == Event::Character('\x04') || e == Event::Character('\x13'))
    return true;
  auto in = e.input();
  return in == "\x1b[13;5u" || in == "\x1b[13;5~";
}

bool is_newline_key(const Event& e) {
  auto in = e.input();
  if (in == "\x1b[13;2u" || in == "\x1b[13;2~" || in == "\x1b[27;2;13~" ||
      in == "\x1b\r" || in == "\x1b\n")
    return true;
  if (in == "\x1bj" || in == "\x1bJ" || in == "\x1b[106;3u" ||
      in == "\x1b[74;3u")
    return true;
  if (e.is_character()) {
    auto ch = e.character();
    if (ch == "∆") return true;
  }
  return false;
}

bool is_wheel_up(Event e) {
  return e.is_mouse() && e.mouse().button == Mouse::WheelUp;
}

bool is_wheel_down(Event e) {
  return e.is_mouse() && e.mouse().button == Mouse::WheelDown;
}

std::string trim_copy(std::string s) {
  while (!s.empty() && (s.back() == ' ' || s.back() == '\n' || s.back() == '\r'))
    s.pop_back();
  size_t i = 0;
  while (i < s.size() && (s[i] == ' ' || s[i] == '\n' || s[i] == '\r')) ++i;
  return s.substr(i);
}

std::pair<std::string, std::string> split_slash(const std::string& prompt) {
  auto space = prompt.find(' ');
  auto cmd = space == std::string::npos ? prompt : prompt.substr(0, space);
  auto arg = space == std::string::npos ? std::string()
                                        : trim_copy(prompt.substr(space + 1));
  for (char& c : cmd)
    if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
  return {cmd, arg};
}

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
    {"/compact", "/compact", "summarize older session history"},
    {"/reload", "/reload", "reload trusted project resources"},
    {"/skill:", "/skill:NAME [request]", "load a skill"},
    {"/quit", "/quit", "exit"},
    {"/exit", "/exit", "exit"},
};

bool is_builtin_slash(std::string_view command) {
  for (const auto& spec : kSlash)
    if (command == spec.name) return true;
  return false;
}

struct Suggestion {
  std::string fill;
  std::string label;
  bool file = false;
};

bool starts_with(std::string_view s, std::string_view p) {
  return s.size() >= p.size() && s.substr(0, p.size()) == p;
}

std::string lower_copy(std::string s) {
  for (char& c : s)
    if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
  return s;
}

bool contains_ci(std::string_view s, std::string_view p) {
  return lower_copy(std::string(s)).find(lower_copy(std::string(p))) !=
         std::string::npos;
}

std::string session_title(const SessionInfo& info) {
  return !info.name.empty() ? info.name
                            : (!info.preview.empty() ? info.preview : "(empty)");
}

bool session_matches_info(const SessionInfo& info, const std::string& query) {
  return contains_ci(info.id, query) || contains_ci(info.name, query) ||
         contains_ci(info.preview, query);
}

void add_unique(std::vector<std::string>& ids, const std::string& id) {
  if (id.empty()) return;
  for (const auto& x : ids)
    if (x == id) return;
  ids.push_back(id);
}

void sort_suggestions(std::vector<Suggestion>& out) {
  std::sort(out.begin(), out.end(), [](const Suggestion& a, const Suggestion& b) {
    return lower_copy(a.fill) < lower_copy(b.fill);
  });
}

std::vector<Suggestion> suggest_models(const std::string& query,
                                       std::string_view provider,
                                       const std::vector<std::string>& recents) {
  constexpr int kMin = 2;
  constexpr int kCap = 50;
  std::vector<Suggestion> out;
  std::vector<std::string> used;
  auto add = [&](const std::string& id, int context) {
    if (id.empty()) return;
    auto key = lower_copy(id);
    for (const auto& x : used)
      if (x == key) return;
    used.push_back(key);
    auto label = id;
    auto ctx = format_context_k(context);
    if (!ctx.empty()) label += "  " + ctx;
    out.push_back({"/model " + id, std::move(label)});
  };
  auto q = lower_copy(query);
  if (static_cast<int>(q.size()) >= kMin) {
    auto catalog = search_catalog(provider, query, kCap);
    if (!catalog.empty()) {
      for (const auto& row : catalog) add(row.id, row.context);
      return out;
    }
  }
  for (const auto& id : recents) {
    if (!q.empty() && !contains_ci(id, q)) continue;
    add(id, 0);
  }
  if (static_cast<int>(q.size()) < kMin && !q.empty()) return out;
  int remain = kCap - static_cast<int>(out.size());
  if (remain <= 0) return out;
  for (const auto& row : search_catalog(provider, query, remain, recents))
    add(row.id, row.context);
  return out;
}

std::vector<Suggestion> slash_suggestions(const std::string& draft,
                                          const std::filesystem::path& dir,
                                          const std::string& workspace,
                                          std::string_view provider,
                                          std::string_view model,
                                          const std::vector<std::string>& recents,
                                          const std::vector<ExtensionCommand>&
                                              extension_commands) {
  if (draft.empty() || draft[0] != '/' || draft.find('\n') != std::string::npos)
    return {};
  auto [cmd, arg] = split_slash(draft);
  bool trailing = !draft.empty() && (draft.back() == ' ' || draft.back() == '\t');

  if (starts_with(cmd, "/skill:")) {
    std::vector<Suggestion> out;
    auto query = cmd.substr(7);
    for (const auto& skill : discover_skills(workspace)) {
      if (!contains_ci(skill.name, query)) continue;
      out.push_back({"/skill:" + skill.name + " ",
                     "/skill:" + skill.name +
                         (skill.description.empty() ? "" : "  " + skill.description)});
    }
    sort_suggestions(out);
    return out;
  }

  if (cmd == "/resume" && (trailing || !arg.empty())) {
    std::vector<Suggestion> out;
    try {
      for (const auto& info : list_sessions(dir, workspace)) {
        if (!arg.empty() && !session_matches_info(info, arg)) continue;
        out.push_back({"/resume " + info.id, info.id + "  " + session_title(info)});
        if (out.size() == 8) break;
      }
    } catch (...) {
    }
    if (!out.empty()) return out;
  }

  if (cmd == "/restore" && (trailing || !arg.empty())) {
    std::vector<Suggestion> out;
    try {
      for (const auto& info : list_deleted_sessions(dir)) {
        if (!arg.empty() && !session_matches_info(info, arg)) continue;
        out.push_back({"/restore " + info.id, info.id + "  " + session_title(info)});
        if (out.size() == 8) break;
      }
    } catch (...) {
    }
    if (!out.empty()) return out;
  }

  if (cmd == "/model" && (trailing || !arg.empty())) {
    auto models = suggest_models(arg, provider, recents);
    if (!models.empty()) return models;
  }

  if (cmd == "/thinking" && (trailing || !arg.empty())) {
    std::vector<Suggestion> out;
    for (const auto& level : thinking_choices(provider, model)) {
      if (!arg.empty() && !starts_with(level, arg)) continue;
      out.push_back({"/thinking " + level, level});
    }
    if (!out.empty()) return out;
  }

  if (cmd == "/theme" && (trailing || !arg.empty())) {
    std::vector<Suggestion> out;
    for (auto mode : {ThemeMode::automatic, ThemeMode::light, ThemeMode::dark}) {
      std::string name = theme_mode_name(mode);
      if (!arg.empty() && !starts_with(name, lower_copy(arg))) continue;
      out.push_back({"/theme " + name, name});
    }
    if (!out.empty()) return out;
  }

  if (cmd == "/models" && (trailing || !arg.empty())) {
    if (arg.empty() || starts_with("refresh", arg))
      return {{"/models refresh", "/models refresh  fetch models.dev"}};
  }

  if (cmd == "/provider" && (trailing || !arg.empty())) {
    std::vector<Suggestion> out;
    for (auto* spec : all_providers()) {
      if (!arg.empty() && !starts_with(spec->name, arg)) continue;
      out.push_back({"/provider " + std::string(spec->name),
                     std::string(spec->name) + "  " + spec->default_model});
    }
    sort_suggestions(out);
    if (!out.empty()) return out;
  }

  if (!arg.empty()) return {};

  std::vector<Suggestion> out;
  for (const auto& spec : kSlash) {
    if (!starts_with(spec.name, cmd)) continue;
    std::string fill = spec.name;
    if (std::string(spec.usage) != spec.name && fill.back() != ':') fill += ' ';
    out.push_back({fill, std::string(spec.usage) + "  " + spec.hint});
  }
  for (const auto& prompt : discover_prompts(workspace)) {
    auto slash = "/" + prompt.name;
    if (is_builtin_slash(lower_copy(slash))) continue;
    if (!starts_with(lower_copy(slash), cmd)) continue;
    out.push_back({slash + " ", slash +
                                      (prompt.description.empty()
                                           ? std::string()
                                           : "  " + prompt.description)});
  }
  for (const auto& command : extension_commands) {
    auto slash = "/" + command.name;
    if (is_builtin_slash(lower_copy(slash))) continue;
    if (!starts_with(lower_copy(slash), cmd)) continue;
    out.push_back({slash + " ", slash +
                                    (command.description.empty()
                                         ? std::string()
                                         : "  " + command.description)});
  }
  sort_suggestions(out);
  return out;
}

std::string base64_encode(std::string_view in) {
  static constexpr char kTbl[] =
      "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  std::string out;
  int val = 0;
  int valb = -6;
  for (unsigned char c : in) {
    val = (val << 8) + c;
    valb += 8;
    while (valb >= 0) {
      out.push_back(kTbl[(val >> valb) & 0x3F]);
      valb -= 6;
    }
  }
  if (valb > -6) out.push_back(kTbl[((val << 8) >> (valb + 8)) & 0x3F]);
  while (out.size() % 4) out.push_back('=');
  return out;
}

bool pipe_copy(const char* cmd, const std::string& text) {
  FILE* pipe = popen(cmd, "w");
  if (!pipe) return false;
  if (!text.empty()) fwrite(text.data(), 1, text.size(), pipe);
  return pclose(pipe) == 0;
}

void copy_to_clipboard(const std::string& text) {
#if defined(__APPLE__)
  pipe_copy("pbcopy", text);
#elif defined(__linux__)
  if (!pipe_copy("wl-copy", text))
    pipe_copy("xclip -selection clipboard", text);
#endif
  std::cout << "\033]52;c;" << base64_encode(text) << "\a" << std::flush;
}

std::string pipe_read(const char* cmd) {
  FILE* pipe = popen(cmd, "r");
  if (!pipe) return {};
  std::string out;
  char buf[4096];
  while (true) {
    auto n = fread(buf, 1, sizeof(buf), pipe);
    if (n == 0) break;
    out.append(buf, n);
  }
  pclose(pipe);
  return out;
}

std::string paste_from_clipboard() {
  std::string text;
#if defined(__APPLE__)
  text = pipe_read("pbpaste");
#elif defined(__linux__)
  text = pipe_read("wl-paste -n 2>/dev/null");
  if (text.empty()) text = pipe_read("xclip -selection clipboard -o 2>/dev/null");
#endif
  for (auto& c : text)
    if (c == '\r') c = '\n';
  return text;
}

bool is_paste_key(const Event& e) {
  if (e == Event::CtrlV) return true;
  auto in = e.input();
  return in == "\x1b[118;2u" || in == "\x1b[118;5u" || in == "\x1b[118;8u" ||
         in == "\x1b[118;9u";
}

const char* kHelp = R"(/help              this list
/provider          show the current provider
/provider NAME     set the provider and restore its last model
/model             show the current model
/model ID          set the model and save ~/.niminal/config.json
/thinking          show the current reasoning level
/thinking LEVEL    set none|minimal|low|medium|high|xhigh|max
/theme             show the current theme
/theme MODE        set light|dark|auto (auto follows the terminal background)
/permissions       show remembered tool grants
/permissions clear clear project tool grants
/trust             show project resource trust
/trust on|off      enable or disable project resources
/yolo              auto-approve tools for this process
/yolo off          return to normal approval prompts
/models refresh    download the models.dev catalog for /model suggestions
/session           show the current session
/name [title]      show or set the session name
/resume            list recent sessions for this workspace
/resume ID         load a session
/search TEXT       search this workspace's sessions for text
/fork [title]      copy this session into a new session
/export [PATH]     write this session as Markdown, or JSON with a .json path
/delete ID         move a session to ~/.niminal/sessions/.trash
/restore           list deleted sessions
/restore ID        bring a deleted session back
/new               start a new session (old file stays)
/clear             same as /new
/copy              copy the last error or assistant reply
/compact           summarize older history; keep recent turns
/reload            reload trusted project resources
/skill:NAME [text] load a skill and optionally give it a request
/NAME [text]       expand a Markdown prompt template
/quit              exit

Enter sends. While a turn runs, Enter queues a steering message.
Alt-J or Shift-Enter inserts a newline.
Type @ to add a workspace file. Gitignored files and dependency folders are hidden.
Tab accepts a suggestion. Up/Down picks one, or walks prompt history.
Page Up/Down and the trackpad scroll the transcript.
Esc interrupts a running turn, or clears the composer.
Drag to copy. Ctrl-V pastes into the composer. /copy copies the last reply.
Ctrl-C quits.)";

}  // namespace

int run_tui(niminal::Agent& agent, Workspace& workspace,
            Config& cfg, Session& session,
            std::shared_ptr<ExtensionRuntime>& extensions, bool yolo,
            const std::vector<std::string>* allowed_tools) {
  const auto& cwd = workspace.root();
  auto screen = ScreenInteractive::Fullscreen();
  ThemeMode theme_mode = parse_theme_mode(cfg.theme).value_or(ThemeMode::automatic);
  Theme theme = resolve_theme(theme_mode);
  std::atomic<bool> local_cancel{false};
  if (!agent.cancel) agent.cancel = &local_cancel;
  auto* cancel = agent.cancel;

  std::vector<Block> blocks;
  std::mutex file_changes_mu;
  std::unordered_map<std::string, PendingFileChange> file_changes;
  blocks.push_back(Block{
      BlockKind::status,
      "enter send  ·  /help  ·  alt-j newline  ·  esc interrupt/clear  ·  ctrl-c quit",
  });

  std::string draft;
  int cursor = 0;
  std::vector<std::string> history;
  int history_i = -1;
  std::string live_draft;
  std::mutex steering_mu;
  std::vector<std::string> steering;
  std::vector<std::string> follow_up;
  std::vector<std::string> idle_extension_messages;
  std::function<void(std::string)> deliver_extension_now;
  std::vector<ExtensionEntry> extension_entries_pending;
  std::atomic<bool> busy{false};
  std::string activity;
  float transcript_y = 1.f;
  bool pasting = false;
  bool stick_bottom = true;
  std::atomic<bool> ui_alive{true};
  const auto ui_thread = std::this_thread::get_id();
  std::thread worker;
  PermissionPolicy permissions(cwd);
  bool yolo_mode = yolo;
  struct ApprovalGate {
    std::mutex mutex;
    std::condition_variable condition;
    bool pending = false;
    bool resolved = false;
    std::string tool_id;
    bool can_remember = false;
    PermissionDecision decision = PermissionDecision::deny;
  } approval;

  struct UiCall {
    std::mutex mutex;
    std::condition_variable condition;
    bool done = false;
    std::exception_ptr error;
  };
  auto run_on_ui = [&](std::function<void()> callback) {
    if (std::this_thread::get_id() == ui_thread) {
      callback();
      return;
    }
    auto call = std::make_shared<UiCall>();
    screen.Post([call, callback = std::move(callback)] {
      try {
        callback();
      } catch (...) {
        call->error = std::current_exception();
      }
      {
        std::lock_guard lock(call->mutex);
        call->done = true;
      }
      call->condition.notify_one();
    });
    std::unique_lock lock(call->mutex);
    call->condition.wait(lock, [&] { return call->done || !ui_alive; });
    if (!call->done) throw std::runtime_error("interactive UI is unavailable");
    if (call->error) std::rethrow_exception(call->error);
  };

  auto configure_extension_ui = [&](const std::shared_ptr<ExtensionRuntime>& runtime) {
    if (!runtime) return;
    ExtensionUiCallbacks callbacks;
    callbacks.question = [&](const std::string& prompt,
                             const std::vector<std::string>& options) {
      std::string answer;
      run_on_ui([&] {
        screen.WithRestoredIO([&] {
          std::cout << "\n" << prompt;
          if (!options.empty()) {
            std::cout << "\n";
            for (size_t i = 0; i < options.size(); ++i)
              std::cout << "  [" << (i + 1) << "] " << options[i] << "\n";
          }
          std::cout << "> " << std::flush;
          std::getline(std::cin, answer);
          if (!options.empty() && answer.size() == 1 &&
              answer[0] >= '1' &&
              answer[0] <= static_cast<char>('0' + options.size()))
            answer = options[static_cast<size_t>(answer[0] - '1')];
        });
      });
      return answer;
    };
    callbacks.input = [&](const std::string& prompt, bool secret) {
      std::string answer;
      run_on_ui([&] {
        screen.WithRestoredIO([&] {
          std::cout << "\n" << prompt << ": " << std::flush;
          termios old_termios{};
          const bool hidden = secret && tcgetattr(STDIN_FILENO, &old_termios) == 0;
          if (hidden) {
            auto hidden_termios = old_termios;
            hidden_termios.c_lflag &= static_cast<unsigned long>(~ECHO);
            tcsetattr(STDIN_FILENO, TCSANOW, &hidden_termios);
          }
          std::getline(std::cin, answer);
          if (hidden) {
            tcsetattr(STDIN_FILENO, TCSANOW, &old_termios);
            std::cout << "\n";
          }
        });
      });
      return answer;
    };
    callbacks.editor = [&](const std::string&, const std::string& text) {
      std::string edited;
      run_on_ui([&] {
        screen.WithRestoredIO([&] { edited = edit_text_externally(text); });
      });
      return edited;
    };
    runtime->set_ui_callbacks(std::move(callbacks));
  };
  int suggest_i = 0;
  std::string suggest_sig;
  std::filesystem::path session_dir;
  try {
    session_dir = default_session_dir();
  } catch (...) {
  }

  auto current_suggestions = [&] {
    std::vector<std::string> recents;
    add_unique(recents, agent.model);
    if (auto it = cfg.last_models.find(agent.provider); it != cfg.last_models.end())
      add_unique(recents, it->second);
    add_unique(recents, cfg.model);
    std::vector<Suggestion> items;
    if (auto mention = file_mention_at(draft, static_cast<size_t>(cursor))) {
      for (const auto& path : suggest_mentioned_files(workspace, mention->query))
        items.push_back({path, "@" + path, true});
    } else {
      static const std::vector<ExtensionCommand> no_commands;
      const auto& commands = extensions ? extensions->commands() : no_commands;
      items = slash_suggestions(draft, session_dir, cwd.string(), agent.provider,
                                agent.model, recents, commands);
    }
    std::string sig;
    for (const auto& item : items) {
      sig += item.fill;
      sig += '\n';
    }
    if (sig != suggest_sig) {
      suggest_sig = std::move(sig);
      suggest_i = 0;
    }
    if (!items.empty())
      suggest_i =
          std::clamp(suggest_i, 0, static_cast<int>(items.size()) - 1);
    return items;
  };

  auto apply_suggestion = [&](const Suggestion& item) {
    if (item.file) {
      auto mention = file_mention_at(draft, static_cast<size_t>(cursor));
      draft = apply_file_mention(draft, static_cast<size_t>(cursor), item.fill);
      cursor = mention ? static_cast<int>(mention->start + item.fill.size() + 2)
                       : static_cast<int>(draft.size());
    } else {
      draft = item.fill;
      cursor = static_cast<int>(draft.size());
    }
    history_i = -1;
  };

  auto load_history = [&] {
    history.clear();
    for (const auto& event : session.events) {
      if (!event.is_object() || event.value("type", "") != "user") continue;
      std::string text;
      for (const auto& part : event.value("content", json::array()))
        if (part.is_object() && part.value("type", "") == "text")
          text += part.value("text", "");
      if (!text.empty()) history.push_back(std::move(text));
    }
    if (history.size() > 500)
      history.erase(history.begin(), history.end() - 500);
    history_i = -1;
    live_draft.clear();
  };
  load_history();

  auto remember_input = [&](const std::string& text) {
    if (text.empty() || text[0] == '/') return;
    if (history.empty() || history.back() != text) history.push_back(text);
    if (history.size() > 500)
      history.erase(history.begin(),
                    history.begin() + static_cast<std::ptrdiff_t>(history.size() - 500));
    history_i = -1;
  };

  auto history_prev = [&] {
    if (history.empty()) return;
    if (history_i < 0) {
      live_draft = draft;
      history_i = static_cast<int>(history.size()) - 1;
    } else if (history_i > 0) {
      --history_i;
    }
    draft = history[static_cast<size_t>(history_i)];
    cursor = static_cast<int>(draft.size());
  };

  auto history_next = [&] {
    if (history_i < 0) return;
    if (history_i + 1 < static_cast<int>(history.size())) {
      ++history_i;
      draft = history[static_cast<size_t>(history_i)];
    } else {
      history_i = -1;
      draft = live_draft;
    }
    cursor = static_cast<int>(draft.size());
  };

  auto apply_extension_actions = [&] {
    if (!extensions) return;
    extensions->pump();
    for (auto& notice : extensions->take_notices())
      blocks.push_back(Block{
          notice.level == "error" ? BlockKind::error : BlockKind::status,
          std::move(notice.message)});
    auto entries = extensions->take_entries();
    extension_entries_pending.insert(extension_entries_pending.end(),
                                     std::make_move_iterator(entries.begin()),
                                     std::make_move_iterator(entries.end()));
    if (!busy) {
      for (const auto& entry : extension_entries_pending)
        session.add_extension(entry.extension, entry.data);
      extension_entries_pending.clear();
    }
    for (auto& message : extensions->take_user_messages()) {
      if (!busy && deliver_extension_now) {
        deliver_extension_now(std::move(message.content));
      } else if (!busy) {
        idle_extension_messages.push_back(std::move(message.content));
      } else {
        std::lock_guard lock(steering_mu);
        if (message.deliver_as == "steer") steering.push_back(message.content);
        else follow_up.push_back(message.content);
        blocks.push_back(Block{BlockKind::status,
                               "Extension queued (" + message.deliver_as +
                                   "): " + message.content});
      }
    }
  };

  auto apply_event = [&](StreamEvent ev) {
    switch (ev.kind) {
      case EventKind::text_delta:
        if (blocks.empty() || blocks.back().kind != BlockKind::assistant)
          blocks.push_back(Block{BlockKind::assistant, {}});
        blocks.back().text += ev.text;
        activity = "Responding…";
        break;
      case EventKind::thinking_delta:
        if (blocks.empty() || blocks.back().kind != BlockKind::thinking)
          blocks.push_back(Block{BlockKind::thinking, {}});
        blocks.back().text += ev.text;
        activity = "Thinking…";
        break;
      case EventKind::tool_output_delta:
        break;
      case EventKind::tool_call:
        blocks.push_back(
            Block{BlockKind::tool, tool_summary(ev.tool_name, ev.text)});
        activity = ev.tool_name.empty() ? "Waiting for model…"
                                        : "Running " + ev.tool_name + "…";
        break;
      case EventKind::approval_required:
        blocks.push_back(Block{
            BlockKind::approval,
            ev.tool_name + "\n  Allow " + ev.tool_name +
                (ev.text.empty() ? std::string() : ": " + ev.text) +
                "\n  [enter] once  [s] session" +
                (ev.can_remember ? "  [p] project" : "") +
                "  [n] deny"});
        activity = "Approval needed";
        break;
      case EventKind::tool_result:
        if (!ev.tool_id.empty()) {
          std::optional<PendingFileChange> pending;
          {
            std::lock_guard lock(file_changes_mu);
            auto it = file_changes.find(ev.tool_id);
            if (it != file_changes.end()) {
              pending = std::move(it->second);
              file_changes.erase(it);
            }
          }
          if (pending && !ev.is_error) {
            std::error_code ec;
            const bool after_exists =
                std::filesystem::is_regular_file(pending->path, ec);
            auto after = after_exists ? read_text_file(pending->path)
                                      : std::optional<std::string>{};
            if (!after_exists || after) {
              auto diff = make_file_diff(
                  pending->before_exists, pending->before, after_exists,
                  after.value_or(std::string()));
              if (diff.changed)
                blocks.push_back(Block{BlockKind::diff, std::move(diff.body),
                                       std::move(pending->relative),
                                       diff.created});
            }
          }
        }
        activity = "Waiting for model…";
        break;
      case EventKind::user:
        blocks.push_back(Block{BlockKind::user, ev.text});
        break;
      case EventKind::status:
        if (!ev.text.empty())
          blocks.push_back(Block{BlockKind::status, ev.text});
        break;
      case EventKind::error:
        blocks.push_back(Block{BlockKind::error, ev.text});
        busy = false;
        activity.clear();
        break;
      case EventKind::done:
        busy = false;
        activity.clear();
        break;
      case EventKind::run_start:
      case EventKind::step_start:
      case EventKind::step_end:
      case EventKind::run_end:
      case EventKind::assistant_message:
        break;
    }
    if (stick_bottom) transcript_y = 1.f;
    if (blocks.size() > 80)
      blocks.erase(blocks.begin(),
                   blocks.begin() + static_cast<std::ptrdiff_t>(blocks.size() - 80));
  };

  auto post_ui = [&](StreamEvent ev) {
    if (ev.kind == EventKind::tool_call &&
        (ev.tool_name == "edit" || ev.tool_name == "write")) {
      try {
        auto path_arg = ev.input.is_object()
                            ? ev.input.value("path", std::string())
                            : std::string();
        if (!path_arg.empty()) {
          auto path = workspace.resolve(path_arg);
          auto relative = workspace.relative(path);
          std::error_code ec;
          const bool before_exists =
              std::filesystem::is_regular_file(path, ec);
          auto before = before_exists ? read_text_file(path)
                                      : std::optional<std::string>{};
          if (!before_exists || before) {
            std::lock_guard lock(file_changes_mu);
            file_changes[ev.tool_id] = PendingFileChange{
                std::move(path), std::move(relative), before_exists,
                before.value_or(std::string())};
          }
        }
      } catch (...) {
      }
    }
    if (!ui_alive) return;
    screen.Post([apply_event, apply_extension_actions, ev, &screen] {
      try {
        apply_event(ev);
        apply_extension_actions();
      } catch (const std::exception& e) {
        try {
          apply_event(StreamEvent{EventKind::error, e.what(), {}, {}});
        } catch (...) {
        }
      }
      screen.RequestAnimationFrame();
    });
  };

  agent.on_event = [&](const StreamEvent& ev) { post_ui(ev); };
  auto configure_extension_updates =
      [&](const std::shared_ptr<ExtensionRuntime>& runtime) {
        if (!runtime) return;
        runtime->set_tool_update([&post_ui](const std::string&,
                                            const std::string& content) {
          post_ui(StreamEvent{EventKind::status, content, {}, {}});
        });
      };
  configure_extension_updates(extensions);
  configure_extension_ui(extensions);
  bind_extensions(agent, extensions, cwd, [&](const std::string& warning) {
    post_ui(StreamEvent{EventKind::status, warning, {}, {}});
  }, &session, cfg);
  agent.approve_tool = [&](const niminal::ToolCall& call,
                           const niminal::Tool&) {
    if (yolo_mode) return true;
    auto check = permissions.check(call);
    if (check == PermissionCheck::allow) return true;
    if (check == PermissionCheck::deny) return false;

    {
      std::lock_guard lock(approval.mutex);
      approval.pending = true;
      approval.resolved = false;
      approval.tool_id = call.id;
      approval.can_remember = can_remember(call);
      approval.decision = PermissionDecision::deny;
    }
    StreamEvent request{EventKind::approval_required,
                        permission_description(call), call.name, call.id};
    request.input = json::object();
    try {
      if (!call.arguments.empty()) request.input = json::parse(call.arguments);
    } catch (...) {
    }
    request.can_remember = approval.can_remember;
    post_ui(std::move(request));

    PermissionDecision decision = PermissionDecision::deny;
    while (true) {
      std::unique_lock lock(approval.mutex);
      if (approval.resolved) {
        decision = approval.decision;
        approval.pending = false;
        break;
      }
      if (!ui_alive || cancel->load()) {
        approval.pending = false;
        break;
      }
      approval.condition.wait_for(lock, std::chrono::milliseconds(50));
    }
    try {
      permissions.remember(call, decision);
    } catch (const std::exception& e) {
      post_ui(StreamEvent{EventKind::status,
                           "Could not save permission grant: " +
                               std::string(e.what()),
                           {}, {}});
    }
    return decision != PermissionDecision::deny;
  };
  bind_compaction(agent, session, [&](const std::string& msg) {
    if (msg.empty()) return;
    post_ui(StreamEvent{EventKind::status, msg, {}, {}});
  }, extensions, cfg);
  agent.take_steering = [&] {
    std::lock_guard<std::mutex> lock(steering_mu);
    auto out = std::move(steering);
    steering.clear();
    return out;
  };
  agent.take_follow_up = [&] {
    std::lock_guard<std::mutex> lock(steering_mu);
    auto out = std::move(follow_up);
    follow_up.clear();
    return out;
  };

  auto join_worker = [&] {
    if (worker.joinable()) worker.join();
  };

  auto restart_extensions = [&](bool end_current = true) {
    if (extensions && end_current) {
      auto ended = extensions->dispatch(
          HookEvent::session_end,
          session_hook_payload(session.id, cwd));
      for (const auto& warning : ended.warnings)
        blocks.push_back(Block{BlockKind::status, warning});
    }
    if (extensions) extensions->stop();
    extensions = ExtensionRuntime::start(cwd, session.id, cancel);
    install_extension_tools(agent, extensions, allowed_tools);
    configure_extension_updates(extensions);
    configure_extension_ui(extensions);
    bind_extensions(agent, extensions, cwd, [&](const std::string& warning) {
      post_ui(StreamEvent{EventKind::status, warning, {}, {}});
    }, &session, cfg);
    bind_compaction(agent, session, [&](const std::string& msg) {
      if (!msg.empty()) post_ui(StreamEvent{EventKind::status, msg, {}, {}});
    }, extensions, cfg);
    for (const auto& warning : extensions->warnings())
      blocks.push_back(Block{BlockKind::status, warning});
    auto started = extensions->dispatch(
        HookEvent::session_start,
        session_hook_payload(session.id, cwd));
    for (const auto& warning : started.warnings)
      blocks.push_back(Block{BlockKind::status, warning});
    apply_extension_actions();
  };

  auto load_into_ui = [&](const std::string& note) {
    blocks.clear();
    if (!note.empty())
      blocks.push_back(Block{BlockKind::status, note});
    if (!session.workspace.empty() &&
        session.workspace != cwd.string()) {
      blocks.push_back(Block{
          BlockKind::status,
          "This session was started in " + session.workspace});
    }
    for (const auto& event : session.events) {
      if (!event.is_object()) continue;
      auto type = event.value("type", "");
      if (type == "user") {
        std::string text;
        if (event.contains("content") && event["content"].is_array())
          for (const auto& part : event["content"])
            if (part.is_object() && part.value("type", "") == "text")
              text += part.value("text", "");
        if (!text.empty())
          blocks.push_back(Block{BlockKind::user, std::move(text)});
      } else if (type == "assistant") {
        std::string text;
        if (event.contains("content") && event["content"].is_array()) {
          for (const auto& part : event["content"]) {
            if (!part.is_object()) continue;
            auto ptype = part.value("type", "");
            if (ptype == "text") text += part.value("text", "");
            if (ptype == "tool_use") {
              if (!text.empty()) {
                blocks.push_back(Block{BlockKind::assistant, text});
                text.clear();
              }
              json input = part.value("input", json::object());
              blocks.push_back(Block{
                  BlockKind::tool,
                  tool_summary(part.value("name", ""),
                               input.is_object() ? input.dump() : std::string())});
            }
          }
        }
        if (!text.empty())
          blocks.push_back(Block{BlockKind::assistant, std::move(text)});
      } else if (type == "compaction") {
        auto summary = event.value("summary", "");
        blocks.push_back(Block{
            BlockKind::status,
            "Compacted earlier turns.\n" + clip_text(summary, 1200, 12)});
      }
    }
    if (blocks.empty())
      blocks.push_back(Block{
          BlockKind::status,
          "enter send  ·  /help  ·  alt-j newline  ·  esc interrupt/clear  ·  ctrl-c quit"});
    if (yolo_mode)
      blocks.push_back(Block{BlockKind::status,
                             "YOLO mode: all tools auto-approved for this process."});
  };

  auto adopt_session = [&](Session next, const std::string& note) {
    if (extensions) {
      auto ended = extensions->dispatch(
          HookEvent::session_end,
          session_hook_payload(session.id, cwd));
      for (const auto& warning : ended.warnings)
        blocks.push_back(Block{BlockKind::status, warning});
      extensions->stop();
    }
    session = std::move(next);
    load_history();
    bind_session(agent, session);
    if (auto p = session.last_provider(); find_provider(p)) {
      cfg.provider = p;
      cfg.api_url = find_provider(p)->endpoint;
    }
    if (auto model = session.last_model(); !model.empty())
      cfg.model = model;
    apply_provider(agent, cfg);
    agent.messages = session.openai_messages();
    load_into_ui(note);
    restart_extensions(false);
  };

  bind_session(agent, session);
  int recovered = 0;
  try {
    recovered = session.recover_interrupted_tools();
  } catch (const std::exception& e) {
    blocks.push_back(Block{BlockKind::error, e.what()});
  }
  agent.messages = session.openai_messages();
  load_into_ui(recovered ? "Recovered " + std::to_string(recovered) +
                               " interrupted tool call(s)."
                         : "");
  if (extensions) {
    for (const auto& warning : extensions->warnings())
      blocks.push_back(Block{BlockKind::status, warning});
    apply_extension_actions();
  }

  auto send_prompt = [&](std::string prompt) {
    prompt = trim_copy(std::move(prompt));
    if (prompt.empty()) return;
    remember_input(prompt);
    if (busy) {
      {
        std::lock_guard<std::mutex> lock(steering_mu);
        steering.push_back(prompt);
      }
      blocks.push_back(Block{BlockKind::status, "Queued: " + prompt});
      return;
    }
    join_worker();
    cancel->store(false);
    blocks.push_back(Block{BlockKind::user, prompt});
    busy = true;
    activity = "Thinking…";
    worker = std::thread([&agent, &busy, &ui_alive, post_ui,
                          &workspace, prompt = std::move(prompt)] {
      try {
        agent.run(expand_file_mentions(workspace, prompt));
      } catch (const std::exception& e) {
        if (ui_alive) {
          post_ui(StreamEvent{EventKind::error, e.what(), {}, {}});
        } else {
          busy = false;
        }
      }
    });
  };
  deliver_extension_now = [&](std::string prompt) {
    send_prompt(std::move(prompt));
  };
  for (auto& message : idle_extension_messages)
    deliver_extension_now(std::move(message));
  idle_extension_messages.clear();

  auto start_turn = [&](std::string prompt) {
    prompt = trim_copy(std::move(prompt));
    if (prompt.empty()) return;
    stick_bottom = true;
    transcript_y = 1.f;

    auto initial_cmd = split_slash(prompt).first;
    bool skill_request = starts_with(initial_cmd, "/skill:");
    bool extension_request = false;
    if (extensions) {
      for (const auto& command : extensions->commands())
        if (lower_copy("/" + command.name) == initial_cmd) {
          extension_request = true;
          break;
        }
    }
    if (skill_request) {
      auto name = initial_cmd.substr(7);
      auto skills = discover_skills(cwd);
      if (name.empty() || std::none_of(skills.begin(), skills.end(),
                                      [&](const Skill& skill) {
                                        return skill.name == name;
                                      })) {
        blocks.push_back(Block{BlockKind::error, "unknown skill " + name});
        return;
      }
    }

    if (prompt[0] == '/' && !skill_request && !extension_request) {
      auto [cmd, arg] = split_slash(prompt);
      if (!is_builtin_slash(cmd)) {
        if (auto template_prompt = load_prompt(cwd, cmd.substr(1))) {
          auto expanded = expand_prompt(cwd, template_prompt->name, arg);
          send_prompt(expanded.empty() ? prompt : std::move(expanded));
          return;
        }
      }
    }

    if (prompt[0] == '/' && !skill_request) {
      auto [cmd, arg] = split_slash(prompt);
      if (cmd == "/quit" || cmd == "/exit") {
        cancel->store(true);
        ui_alive = false;
        screen.Exit();
        return;
      }
      if (cmd == "/copy") {
        if (!arg.empty()) {
          blocks.push_back(Block{BlockKind::error, "/copy takes no arguments"});
          return;
        }
        std::string text;
        for (auto it = blocks.rbegin(); it != blocks.rend(); ++it) {
          if ((it->kind == BlockKind::error || it->kind == BlockKind::assistant) &&
              !it->text.empty()) {
            text = it->text;
            break;
          }
        }
        if (text.empty()) text = session.last_assistant_text();
        if (text.empty()) {
          blocks.push_back(
              Block{BlockKind::status, "Nothing to copy yet."});
          return;
        }
        copy_to_clipboard(text);
        blocks.push_back(Block{BlockKind::status, "Copied to clipboard."});
        return;
      }
      if (cmd == "/yolo") {
        if (arg.empty() || arg == "on") {
          yolo_mode = true;
          blocks.push_back(Block{
              BlockKind::status,
              "YOLO mode: all tools auto-approved for this process."});
        } else if (arg == "off") {
          yolo_mode = false;
          blocks.push_back(Block{BlockKind::status,
                                 "YOLO mode disabled; tool approvals are on."});
        } else {
          blocks.push_back(Block{BlockKind::error, "Usage: /yolo [off]"});
        }
        return;
      }
      if (cmd == "/compact") {
        if (busy) {
          blocks.push_back(Block{
              BlockKind::status,
              "wait for the turn to finish, or Esc to interrupt"});
          return;
        }
        try {
          auto result =
              compact_session(session, agent, arg, extensions, cfg);
          agent.messages = session.openai_messages();
          for (const auto& warning : result.warnings)
            blocks.push_back(Block{BlockKind::status, warning});
          blocks.push_back(Block{BlockKind::status, result.message});
        } catch (const std::exception& e) {
          blocks.push_back(Block{BlockKind::error, e.what()});
        }
        return;
      }
      if (busy) {
        blocks.push_back(Block{
            BlockKind::status,
            "wait for the turn to finish, or Esc to interrupt"});
        return;
      }
      if (cmd == "/help") {
        blocks.push_back(Block{BlockKind::status, kHelp});
        return;
      }
      if (extension_request) {
        try {
          json context = {
              {"mode", "tui"},
              {"workspace", cwd.string()},
              {"session_id", session.id},
              {"provider", agent.provider},
              {"model", agent.model},
              {"messages", session.openai_messages()},
          };
          auto response = extensions->invoke(cmd.substr(1), arg, context);
          auto message = response.value("message", std::string());
          if (!message.empty())
            blocks.push_back(Block{BlockKind::status, std::move(message)});
          apply_extension_actions();
          bool restarted = false;
          if (auto action = response.find("session");
              action != response.end() && action->is_object()) {
            auto kind = action->value("action", std::string());
            if (kind == "new") {
              auto next = create_session(default_session_dir(), cwd.string());
              next.persist = session.persist;
              if (!next.persist) next.path.clear();
              adopt_session(std::move(next), "New session");
              restarted = true;
            } else if (kind == "switch") {
              auto id = action->value("id", std::string());
              auto next = load_session(default_session_dir(), id);
              next.recover_interrupted_tools();
              adopt_session(std::move(next), "Resumed " + id);
              restarted = true;
            } else if (kind == "compact") {
              auto compacted = compact_session(
                  session, agent,
                  action->value("instruction", std::string()), extensions, cfg);
              agent.messages = session.openai_messages();
              blocks.push_back(Block{BlockKind::status, compacted.message});
            }
            auto editor_text = action->value("editor_text", std::string());
            if (!editor_text.empty()) {
              draft = std::move(editor_text);
              cursor = static_cast<int>(draft.size());
            }
          }
          if (response.value("reload", false) && !restarted)
            restart_extensions();
          auto next_prompt = response.value("prompt", std::string());
          if (!next_prompt.empty()) send_prompt(std::move(next_prompt));
        } catch (const std::exception& e) {
          blocks.push_back(Block{BlockKind::error, e.what()});
        }
        return;
      }
      if (cmd == "/reload") {
        if (!arg.empty()) {
          blocks.push_back(Block{BlockKind::error, "/reload takes no arguments"});
          return;
        }
        permissions.reload_project();
        for (auto& tool : agent.tools)
          if (tool.name == "skill") tool = skill_tool(cwd);
        restart_extensions();
        blocks.push_back(Block{BlockKind::status, "Reloaded project resources."});
        return;
      }
      if (cmd == "/permissions") {
        if (arg.empty()) {
          blocks.push_back(Block{BlockKind::status, permissions.describe()});
        } else if (arg == "clear") {
          try {
            permissions.clear_project();
            blocks.push_back(
                Block{BlockKind::status, "Cleared project permission grants."});
          } catch (const std::exception& e) {
            blocks.push_back(Block{BlockKind::error, e.what()});
          }
        } else {
          blocks.push_back(Block{BlockKind::error,
                                 "Usage: /permissions [clear]"});
        }
        return;
      }
      if (cmd == "/trust") {
        auto resources = project_trust_resources(cwd);
        if (resources.empty()) {
          blocks.push_back(Block{
              BlockKind::status, "No project-local resources require trust."});
        } else if (arg.empty()) {
          blocks.push_back(Block{
              BlockKind::status,
              std::string("Project-local resources: ") +
                  (project_resources_trusted(cwd) ? "trusted" : "not trusted")});
        } else if (arg == "on" || arg == "off") {
          const bool trusted = arg == "on";
          set_project_resources_trusted(cwd, trusted);
          try {
            save_project_trust(cwd, trusted);
            permissions.reload_project();
            for (auto& tool : agent.tools)
              if (tool.name == "skill") tool = skill_tool(cwd);
            restart_extensions();
            blocks.push_back(Block{
                BlockKind::status,
                trusted ? "Project-local resources enabled."
                        : "Project-local resources disabled."});
          } catch (const std::exception& e) {
            blocks.push_back(Block{BlockKind::error, e.what()});
          }
        } else {
          blocks.push_back(Block{BlockKind::error, "Usage: /trust [on|off]"});
        }
        return;
      }
      if (cmd == "/provider") {
        if (arg.empty()) {
          blocks.push_back(Block{
              BlockKind::status,
              "provider: " + agent.provider + "\nmodel: " + agent.model +
                  "\nurl: " + agent.api_url + "\nkey: " + agent.key_hint});
          return;
        }
        std::string err;
        if (!select_provider(cfg, arg, &err)) {
          blocks.push_back(Block{BlockKind::error, err});
          return;
        }
        apply_provider(agent, cfg);
        try {
          save_config(cfg);
          session.add_selection(agent.model, agent.provider);
          blocks.push_back(Block{
              BlockKind::status,
              "provider set to " + agent.provider + "\nmodel: " + agent.model +
                  "\nsaved " + config_path().string()});
        } catch (const std::exception& e) {
          blocks.push_back(Block{
              BlockKind::error,
              "provider set for this session, save failed: " +
                  std::string(e.what())});
        }
        return;
      }
      if (cmd == "/model") {
        if (arg == "refresh") {
          blocks.push_back(Block{
              BlockKind::error,
              "Unknown /model option 'refresh'; did you mean /models refresh?"});
          return;
        }
        if (arg.empty()) {
          blocks.push_back(Block{BlockKind::status, "model: " + agent.model +
                                                        "\nurl: " + agent.api_url});
          return;
        }
        agent.model = arg;
        cfg.model = arg;
        cfg.last_models[cfg.provider] = arg;
        apply_provider(agent, cfg);
        try {
          save_config(cfg);
          session.add_selection(agent.model, agent.provider);
          blocks.push_back(Block{BlockKind::status, "model set to " + agent.model +
                                                        "\nsaved " +
                                                        config_path().string()});
        } catch (const std::exception& e) {
          blocks.push_back(Block{
              BlockKind::error,
              "model set for this session, save failed: " + std::string(e.what())});
        }
        return;
      }
      if (cmd == "/thinking") {
        if (arg.empty()) {
          auto status = thinking_status(agent.provider, agent.model, cfg.thinking);
          auto choices = thinking_choices(agent.provider, agent.model);
          std::string msg = "thinking: ";
          if (cfg.thinking.empty())
            msg += status.empty() ? "(provider default)" : status;
          else
            msg += status.empty() ? cfg.thinking : status;
          if (!choices.empty()) {
            msg += "\nlevels: ";
            for (size_t i = 0; i < choices.size(); ++i) {
              if (i) msg += '|';
              msg += choices[i];
            }
          }
          blocks.push_back(Block{BlockKind::status, msg});
          return;
        }
        try {
          cfg.thinking = normalize_thinking(arg);
        } catch (const std::exception& e) {
          blocks.push_back(Block{BlockKind::error, e.what()});
          return;
        }
        apply_provider(agent, cfg);
        auto status = thinking_status(agent.provider, agent.model, cfg.thinking);
        try {
          save_config(cfg);
          blocks.push_back(Block{
              BlockKind::status,
              "thinking set to " + (status.empty() ? cfg.thinking : status) +
                  "\nsaved " + config_path().string()});
        } catch (const std::exception& e) {
          blocks.push_back(Block{
              BlockKind::error,
              "thinking set for this session, save failed: " +
                  std::string(e.what())});
        }
        return;
      }
      if (cmd == "/theme") {
        if (arg.empty()) {
          blocks.push_back(Block{
              BlockKind::status,
              std::string("theme: ") + theme_mode_name(theme_mode) +
                  (theme_mode == ThemeMode::automatic
                       ? std::string(" (") + theme_mode_name(detect_terminal_theme()) +
                             ")"
                       : std::string())});
          return;
        }
        auto mode = parse_theme_mode(arg);
        if (!mode) {
          blocks.push_back(
              Block{BlockKind::error, "Usage: /theme [light|dark|auto]"});
          return;
        }
        theme_mode = *mode;
        theme = resolve_theme(theme_mode);
        cfg.theme = theme_mode_name(theme_mode);
        try {
          save_config(cfg);
          blocks.push_back(Block{BlockKind::status,
                                 std::string("theme set to ") +
                                     theme_mode_name(theme_mode) + "\nsaved " +
                                     config_path().string()});
        } catch (const std::exception& e) {
          blocks.push_back(Block{
              BlockKind::error,
              std::string("theme set for this session, save failed: ") + e.what()});
        }
        return;
      }
      if (cmd == "/models") {
        if (arg != "refresh") {
          blocks.push_back(Block{BlockKind::error, "Usage: /models refresh"});
          return;
        }
        if (refresh_catalog()) {
          blocks.push_back(Block{
              BlockKind::status,
              "Updated model catalog  ·  " + catalog_cache_path().string()});
        } else {
          blocks.push_back(Block{
              BlockKind::error,
              "Could not refresh model metadata; using existing cache."});
        }
        return;
      }
      if (cmd == "/session") {
        blocks.push_back(Block{BlockKind::status, session.describe()});
        return;
      }
      if (cmd == "/name") {
        if (arg.empty()) {
          blocks.push_back(Block{
              BlockKind::status,
              session.name.empty() ? "Name: (none)" : "Name: " + session.name});
          return;
        }
        session.add_name(arg);
        blocks.push_back(Block{BlockKind::status, "Name: " + session.name});
        return;
      }
      if (cmd == "/resume") {
        auto dir = default_session_dir();
        if (arg.empty() || !valid_session_id(arg)) {
          auto infos = search_sessions(dir, cwd.string(), arg);
          blocks.push_back(
              Block{BlockKind::status, format_session_list(infos, session.id)});
          return;
        }
        try {
          auto next = load_session(dir, arg);
          next.recover_interrupted_tools();
          adopt_session(std::move(next), "Resumed " + arg);
        } catch (const std::exception& e) {
          blocks.push_back(Block{BlockKind::error, e.what()});
        }
        return;
      }
      if (cmd == "/search") {
        if (arg.empty()) {
          blocks.push_back(Block{BlockKind::error, "Usage: /search TEXT"});
          return;
        }
        auto infos = search_sessions(default_session_dir(), cwd.string(), arg);
        blocks.push_back(Block{
            BlockKind::status,
            format_session_list(infos, session.id, "Matches for " + arg)});
        return;
      }
      if (cmd == "/fork") {
        try {
          auto next = session.fork(default_session_dir());
          if (!arg.empty()) next.add_name(arg);
          auto id = next.id;
          adopt_session(std::move(next), "Forked " + id);
        } catch (const std::exception& e) {
          blocks.push_back(Block{BlockKind::error, e.what()});
        }
        return;
      }
      if (cmd == "/export") {
        auto path = arg.empty() ? cwd / (session.id + ".md")
                                : std::filesystem::path(arg);
        try {
          if (path.has_parent_path())
            std::filesystem::create_directories(path.parent_path());
          std::ofstream out(path, std::ios::binary | std::ios::trunc);
          if (!out) throw std::runtime_error("cannot write " + path.string());
          out << session.export_text(path.extension() == ".json" ? "json" : "md");
          blocks.push_back(Block{
              BlockKind::status,
              "Exported " + std::to_string(session.events.size()) + " events to " +
                  path.string()});
        } catch (const std::exception& e) {
          blocks.push_back(Block{BlockKind::error, e.what()});
        }
        return;
      }
      if (cmd == "/delete") {
        if (arg.empty()) {
          blocks.push_back(Block{BlockKind::error,
                                 "Usage: /delete ID  ·  /restore lists deleted sessions"});
        } else if (arg == session.id) {
          blocks.push_back(
              Block{BlockKind::error, "Cannot delete the current session."});
        } else if (delete_session(default_session_dir(), arg)) {
          blocks.push_back(Block{BlockKind::status,
                                 "Deleted " + arg + "  ·  /restore " + arg +
                                     " brings it back"});
        } else {
          blocks.push_back(Block{BlockKind::error, "No session " + arg});
        }
        return;
      }
      if (cmd == "/restore") {
        auto dir = default_session_dir();
        if (arg.empty() || !valid_session_id(arg)) {
          auto infos = list_deleted_sessions(dir);
          blocks.push_back(Block{
              BlockKind::status,
              format_session_list(infos, session.id, "Deleted sessions (newest first)")});
        } else if (restore_session(dir, arg)) {
          blocks.push_back(Block{BlockKind::status,
                                 "Restored " + arg + "  ·  /resume " + arg});
        } else {
          blocks.push_back(Block{BlockKind::error, "No deleted session " + arg});
        }
        return;
      }
      if (cmd == "/clear" || cmd == "/new") {
        auto next = create_session(default_session_dir(), cwd.string());
        next.persist = session.persist;
        if (!next.persist) next.path.clear();
        auto id = next.id;
        adopt_session(std::move(next), "New session " + id);
        apply_provider(agent, cfg);
        return;
      }
      blocks.push_back(
          Block{BlockKind::error, "unknown command " + cmd + "  ·  /help"});
      return;
    }

    send_prompt(std::move(prompt));
  };

  InputOption input_opt;
  input_opt.multiline = true;
  input_opt.cursor_position = &cursor;
  input_opt.transform = [&theme](InputState state) {
    state.element |= color(theme.input_fg);
    if (state.is_placeholder) state.element |= dim;
    if (state.focused)
      state.element |= bgcolor(theme.input_bg);
    else if (state.hovered)
      state.element |= bgcolor(theme.hover_bg);
    return state.element;
  };
  auto input = Input(&draft, "describe a change", input_opt);
  auto input_transform = input_opt.transform;
  auto wrapped_input = Renderer(input, [&] {
    if (draft.empty()) return input->Render();

    const int cursor_pos = std::clamp(cursor, 0, static_cast<int>(draft.size()));
    const int terminal_width = screen.dimx() > 0 ? screen.dimx()
                                                 : Terminal::Size().dimx;
    Elements rows;
    size_t line_start = 0;
    while (true) {
      const auto newline = draft.find('\n', line_start);
      const size_t line_end = newline == std::string::npos ? draft.size()
                                                           : newline;
      const auto line = draft.substr(line_start, line_end - line_start);
      Elements glyphs;
      size_t byte = line_start;
      for (const auto& glyph : Utf8ToGlyphs(line)) {
        auto cell = text(glyph);
        if (!glyph.empty() && byte == static_cast<size_t>(cursor_pos))
          cell = input->Focused() ? focusCursorBarBlinking(std::move(cell))
                                  : focus(std::move(cell));
        glyphs.push_back(std::move(cell));
        byte += glyph.size();
      }
      if (line.empty() || byte == static_cast<size_t>(cursor_pos)) {
        auto cell = text(" ");
        cell = input->Focused() ? focusCursorBarBlinking(std::move(cell))
                                : focus(std::move(cell));
        glyphs.push_back(std::move(cell));
      }
      rows.push_back(hflow(std::move(glyphs)));
      if (line_end == draft.size()) break;
      line_start = line_end + 1;
    }

    auto element = vbox(std::move(rows)) |
                   size(WIDTH, LESS_THAN, std::max(1, terminal_width - 2)) |
                   frame;
    return input_transform({std::move(element), false, input->Focused(), false}) |
           xflex;
  });

  auto layout = Container::Vertical({wrapped_input});
  auto view = Renderer(layout, [&] {
    Elements entries;
    for (const auto& block : blocks) {
      if (block.kind == BlockKind::diff) {
        entries.push_back(render_diff_card(block, theme));
        entries.push_back(separatorEmpty());
        continue;
      }
      auto label = block_label(block.kind);
      auto body = block.kind == BlockKind::assistant
                      ? render_markdown(block.text, theme)
                      : paragraph(block.kind == BlockKind::thinking
                                     ? (cfg.show_thinking
                                            ? block.text
                                            : clip_text(block.text,
                                                        cfg.thinking_preview_chars,
                                                        cfg.thinking_preview_lines))
                                     : block.text) |
                            block_style(block.kind, theme);
      if (label && *label)
        entries.push_back(
            vbox({text(label) | bold | block_style(block.kind, theme), body}));
      else
        entries.push_back(body);
      entries.push_back(separatorEmpty());
    }

    std::string activity_line;
    if (busy || !activity.empty()) {
      if (busy) {
        static const char* kSpin[] = {"⠋", "⠙", "⠹", "⠸", "⠼",
                                      "⠴", "⠦", "⠧", "⠇", "⠏"};
        auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(
                      std::chrono::steady_clock::now().time_since_epoch())
                      .count();
        activity_line = kSpin[(ms / 200) % 10];
        activity_line += ' ';
      }
      activity_line += activity.empty()
                           ? (cancel->load() ? "Stopping…" : "Thinking…")
                           : activity;
      int queued = 0;
      {
        std::lock_guard<std::mutex> lock(steering_mu);
        queued = static_cast<int>(steering.size());
      }
      if (queued > 0)
        activity_line += "  ·  queued " + std::to_string(queued);
    }
    if (extensions) {
      for (const auto& status : extensions->status_texts()) {
        if (!activity_line.empty()) activity_line += "  ·  ";
        activity_line += status;
      }
    }

    auto think = thinking_status(agent.provider, agent.model, cfg.thinking);
    if (think.empty())
      think = thinking_choices(agent.provider, agent.model).empty()
                  ? std::string()
                  : (cfg.thinking.empty() ? "default" : "off");
    std::string usage;
    try {
      usage = format_usage_line(session.usage_totals());
    } catch (...) {
    }

    auto suggestions = current_suggestions();
    Elements suggest_rows;
    for (int i = 0; i < static_cast<int>(suggestions.size()); ++i) {
      auto line = text(suggestions[static_cast<size_t>(i)].label) | dim;
      if (i == suggest_i) line = line | inverted;
      suggest_rows.push_back(std::move(line));
    }

    Elements stack;
    stack.push_back(vbox(std::move(entries)) |
                    focusPositionRelative(0.f, stick_bottom ? 1.f
                                                            : transcript_y) |
                    yframe | vscroll_indicator | yflex);
    stack.push_back(separator());
    if (!suggest_rows.empty())
      stack.push_back(vbox(std::move(suggest_rows)));
    if (extensions) {
      Elements widget_rows;
      for (const auto& line : extensions->widget_lines())
        widget_rows.push_back(text(line) | dim);
      if (!widget_rows.empty()) stack.push_back(vbox(std::move(widget_rows)));
    }
    stack.push_back(text(activity_line.empty() ? " " : activity_line) |
                    color(theme.accent));
    stack.push_back(separatorLight() | dim);
    stack.push_back(hbox({text(busy ? "…" : "› ") | bold,
                          wrapped_input->Render() | xflex |
                              size(HEIGHT, LESS_THAN, 8)}));
    stack.push_back(hbox({
        text(usage.empty() ? "↑0  ↓0" : usage) | dim,
        filler(),
        text(agent.provider + "/" + agent.model +
             (yolo_mode ? " [yolo]" : "")) |
            color(theme.accent),
        text(think.empty() ? std::string() : (":" + think)) | dim,
    }));
    return vbox(std::move(stack));
  });

  auto insert_draft = [&](std::string text) {
    if (text.empty()) return;
    int pos = std::clamp(cursor, 0, static_cast<int>(draft.size()));
    draft.insert(static_cast<size_t>(pos), text);
    cursor = pos + static_cast<int>(text.size());
  };

  auto resolve_approval = [&](PermissionDecision decision) {
    std::lock_guard lock(approval.mutex);
    if (!approval.pending || approval.resolved) return false;
    approval.decision = decision;
    approval.resolved = true;
    approval.condition.notify_all();
    return true;
  };

  auto approval_pending = [&] {
    std::lock_guard lock(approval.mutex);
    return approval.pending && !approval.resolved;
  };

  view = CatchEvent(view, [&](Event e) {
    if (approval_pending()) {
      if (e == Event::Return || e == Event::Character('1')) {
        resolve_approval(PermissionDecision::allow_once);
      } else if (e == Event::Character('s')) {
        resolve_approval(PermissionDecision::allow_session);
      } else if (e == Event::Character('p')) {
        bool allowed = false;
        {
          std::lock_guard lock(approval.mutex);
          allowed = approval.can_remember;
        }
        if (!allowed) return true;
        resolve_approval(PermissionDecision::allow_project);
      } else if (e == Event::Character('n') || e == Event::Escape) {
        resolve_approval(PermissionDecision::deny);
      } else if (e == Event::Character('\x03')) {
        resolve_approval(PermissionDecision::deny);
        cancel->store(true);
        ui_alive = false;
        screen.Exit();
      } else {
        return true;
      }
      return true;
    }
    if (e.input() == "\x1b[200~") {
      pasting = true;
      return true;
    }
    if (e.input() == "\x1b[201~") {
      pasting = false;
      return true;
    }
    if (is_paste_key(e)) {
      insert_draft(paste_from_clipboard());
      return true;
    }
    if (e.is_mouse() && e.mouse().motion == Mouse::Released &&
        e.mouse().button == Mouse::Left) {
      auto sel = screen.GetSelection();
      if (!sel.empty()) {
        copy_to_clipboard(sel);
        return true;
      }
    }
    if (e.is_mouse() && e.mouse().motion == Mouse::Pressed &&
        (e.mouse().button == Mouse::Middle || e.mouse().button == Mouse::Right)) {
      insert_draft(paste_from_clipboard());
      return true;
    }
    if (is_wheel_up(e) || e == Event::PageUp) {
      stick_bottom = false;
      transcript_y = std::max(0.f, transcript_y - (e == Event::PageUp ? 0.35f : 0.07f));
      return true;
    }
    if (is_wheel_down(e) || e == Event::PageDown) {
      transcript_y =
          std::min(1.f, transcript_y + (e == Event::PageDown ? 0.35f : 0.07f));
      if (transcript_y >= 0.99f) {
        transcript_y = 1.f;
        stick_bottom = true;
      }
      return true;
    }
    auto suggestions = current_suggestions();
    if (!suggestions.empty()) {
      if (e == Event::ArrowDown) {
        suggest_i = (suggest_i + 1) % static_cast<int>(suggestions.size());
        return true;
      }
      if (e == Event::ArrowUp) {
        suggest_i = (suggest_i + static_cast<int>(suggestions.size()) - 1) %
                    static_cast<int>(suggestions.size());
        return true;
      }
      if (e == Event::Tab || e == Event::Character('\t')) {
        apply_suggestion(suggestions[static_cast<size_t>(suggest_i)]);
        return true;
      }
      if (e == Event::TabReverse) {
        suggest_i = (suggest_i + static_cast<int>(suggestions.size()) - 1) %
                    static_cast<int>(suggestions.size());
        apply_suggestion(suggestions[static_cast<size_t>(suggest_i)]);
        return true;
      }
    } else {
      if (e == Event::ArrowUp) {
        history_prev();
        return true;
      }
      if (e == Event::ArrowDown) {
        history_next();
        return true;
      }
    }
    if (is_newline_key(e)) {
      int pos = std::clamp(cursor, 0, static_cast<int>(draft.size()));
      draft.insert(static_cast<size_t>(pos), "\n");
      cursor = pos + 1;
      return true;
    }
    if (is_send(e)) {
      if (pasting) {
        insert_draft("\n");
        return true;
      }
      if (!suggestions.empty()) {
        if (suggestions[static_cast<size_t>(suggest_i)].file) {
          apply_suggestion(suggestions[static_cast<size_t>(suggest_i)]);
          return true;
        }
        auto prompt = suggestions[static_cast<size_t>(suggest_i)].fill;
        draft.clear();
        cursor = 0;
        start_turn(std::move(prompt));
        return true;
      }
      auto prompt = draft;
      draft.clear();
      cursor = 0;
      start_turn(std::move(prompt));
      return true;
    }
    if (e == Event::Escape) {
      if (busy) {
        cancel->store(true);
        activity = "Stopping…";
        return true;
      }
      draft.clear();
      cursor = 0;
      history_i = -1;
      return true;
    }
    if (e == Event::Character('\x03')) {
      cancel->store(true);
      ui_alive = false;
      screen.Exit();
      return true;
    }
    return false;
  });

  load_catalog();
  std::thread catalog_thread;
  if (catalog_stale()) {
    catalog_thread = std::thread([&] {
      refresh_catalog();
      if (!ui_alive) return;
      screen.Post([&screen] { screen.RequestAnimationFrame(); });
    });
  }
  std::thread extension_thread([&] {
    while (ui_alive) {
      std::this_thread::sleep_for(std::chrono::milliseconds(200));
      if (!ui_alive) break;
      const bool extension_changed = extensions && extensions->pump();
      if (!busy && !extension_changed) continue;
      screen.Post([apply_extension_actions, &screen] {
        try {
          apply_extension_actions();
        } catch (...) {
        }
        screen.RequestAnimationFrame();
      });
    }
  });

  screen.Post([] { std::cout << "\033[?2004h" << std::flush; });
  screen.Loop(view);
  std::cout << "\033[?2004l" << std::flush;
  ui_alive = false;
  cancel->store(true);
  resolve_approval(PermissionDecision::deny);
  join_worker();
  if (catalog_thread.joinable()) catalog_thread.join();
  extension_thread.join();
  if (extensions) {
    extensions->set_tool_update({});
    extensions->set_ui_callbacks({});
  }
  agent.on_event = {};
  agent.take_steering = {};
  agent.take_follow_up = {};
  agent.before_request = {};
  agent.recover_overflow = {};
  agent.approve_tool = {};
  return 0;
}

}  // namespace niminal::app
