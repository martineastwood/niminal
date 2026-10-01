#include "slash.hpp"

#include "extensions.hpp"
#include "keybindings.hpp"
#include "models_dev.hpp"
#include "prompts.hpp"
#include "session.hpp"
#include "skills.hpp"
#include "theme.hpp"
#include "thinking.hpp"

#include "provider.hpp"
#include <niminal/text.hpp>

#include <algorithm>
#include <cctype>
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
    {"/version", "/version", "show the version"},
    {"/provider", "/provider [name]", "show or set the provider"},
    {"/model", "/model [ID]", "show or set the active model"},
    {"/thinking", "/thinking [level]", "show or set reasoning"},
    {"/theme", "/theme [name]", "show or select a theme"},
    {"/settings", "/settings", "edit config in an overlay"},
    {"/permissions", "/permissions [clear]", "show or clear tool grants"},
    {"/trust", "/trust [on|off]", "show or set project resource trust"},
    {"/yolo", "/yolo [off]", "auto-approve tools for this process"},
    {"/models", "/models [refresh]", "list configured models or refresh the catalog"},
    {"/session", "/session", "show the current session"},
    {"/name", "/name [title]", "show or set the session name"},
    {"/resume", "/resume [ID]", "list or load a session"},
    {"/search", "/search TEXT", "search sessions for text"},
    {"/fork", "/fork [N] [title]", "copy this session, or from user turn N"},
    {"/export", "/export [PATH]", "write this session as Markdown, HTML, or JSON"},
    {"/delete", "/delete ID", "move a session to the trash"},
    {"/restore", "/restore [ID]", "list or restore a deleted session"},
    {"/new", "/new", "start a new session"},
    {"/clear", "/clear", "same as /new"},
    {"/copy", "/copy", "copy the last error or reply"},
    {"/retry", "/retry", "retry the last failed request"},
    {"/compact", "/compact", "summarize older session history"},
    {"/reload", "/reload", "reload trusted project resources"},
    {"/init", "/init", "write or update project instructions"},
    {"/skill:", "/skill:NAME [request]", "load a skill"},
    {"/quit", "/quit", "exit"},
    {"/exit", "/exit", "exit"},
};

constexpr std::string_view kInitPromptHead =
    "Write or update the project instruction file for this repository.\n\nTarget file: ";

constexpr std::string_view kClaudeInPlaceNote =
    "This project has no AGENTS.md. CLAUDE.md already loads as project instructions, so update it "
    "in place instead of creating a second file.\n\n";

constexpr std::string_view kOverrideNote =
    "AGENTS.override.md exists in this directory and takes precedence over this file. Mention that "
    "in your report.\n\n";

constexpr std::string_view kInitPromptBody =
    R"PROMPT(Sessions that start in this directory, or in a directory below it, load this file into their system prompt. It runs before every future request in this project, so every line has to change what an agent does. Concrete beats thorough.

Work in this order.

1. Read the repository before writing. Cover the README, the build manifests (CMakeLists.txt, package.json, pyproject.toml, Makefile, Cargo.toml), the CI workflows, the top two levels of the source layout, and any existing instruction files (AGENTS.md, CLAUDE.md, .cursor/rules, .github/copilot-instructions.md).
2. Run the build, test, lint, format, and check commands you intend to document. Record the exact forms that worked. Never write a command you did not run. When a command matters but you could not run it, mark it unverified.
3. Write only what someone new to the repository cannot infer from the file list:
   - what the project is and its main entry points
   - the commands for the local edit loop, and what to run before pushing
   - the order that matters, such as build before test, or format before check
   - structure and architecture that filenames do not reveal
   - project conventions, setup quirks, and operational gotchas
   - pointers to the other instruction sources you found, so they stay the single source for what they already cover
4. Do not restate the code or explain how it works. A line earns its place by changing the next action an agent takes.
5. Keep it under 100 lines.
6. Update the file in place when it already exists. Keep lines that are still true, fix the ones that are wrong, delete the ones that went stale. Do not reorganize it for its own sake. Call the write tool with overwrite: true.
7. Ask at most two questions with ask_user, and only when the repository cannot answer something that changes the file. Otherwise decide and move on.
8. Finish with a short report: the file you wrote, the lines you added, changed, or removed, the commands you ran and their outcome, and anything you left unverified.
)PROMPT";

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
  if (const auto models = provider_models(provider); !models.empty()) {
    for (const auto& id : models) {
      if (q.empty() || contains_ci(id, q)) {
        add(id, 0);
      }
    }
    return out;
  }
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

std::optional<UserBashRequest> parse_user_bash(std::string_view prompt) {
  if (prompt.empty() || prompt.front() != '!') {
    return std::nullopt;
  }
  UserBashRequest req;
  size_t start = 1;
  if (prompt.size() >= 2 && prompt[1] == '!') {
    req.exclude_from_context = true;
    start = 2;
  }
  req.command = niminal::trim_copy(std::string(prompt.substr(start)));
  if (req.command.empty()) {
    return std::nullopt;
  }
  return req;
}

std::optional<std::pair<size_t, size_t>> user_bash_prefix(std::string_view prompt) {
  const size_t start = prompt.find_first_not_of(" \t\n\r");
  if (start == std::string_view::npos) {
    return std::nullopt;
  }
  auto request = parse_user_bash(prompt.substr(start));
  if (!request) {
    return std::nullopt;
  }
  return std::pair{start, start + (request->exclude_from_context ? 2U : 1U)};
}

std::pair<std::string, std::string> split_slash(const std::string& prompt) {
  auto space = prompt.find(' ');
  auto cmd = space == std::string::npos ? prompt : prompt.substr(0, space);
  auto arg =
      space == std::string::npos ? std::string() : niminal::trim_copy(prompt.substr(space + 1));
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

std::string slash_help(const Keybindings& keybindings) {
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
  auto binding = [&](KeyAction action, std::string_view description) {
    out += keybindings.label(action) + "  " + std::string(description) + '\n';
  };
  out += "\nKeyboard shortcuts (customize in ~/.niminal/keybindings.json):\n";
  binding(KeyAction::submit, "send or queue a message; accept a suggestion");
  binding(KeyAction::newline, "insert a newline");
  binding(KeyAction::word_left, "move left by word");
  binding(KeyAction::word_right, "move right by word");
  binding(KeyAction::draft_start, "jump to draft start");
  binding(KeyAction::draft_end, "jump to draft end");
  binding(KeyAction::previous, "previous suggestion or history entry");
  binding(KeyAction::next, "next suggestion or history entry");
  binding(KeyAction::complete, "accept a suggestion");
  binding(KeyAction::complete_previous, "cycle back and accept a suggestion");
  binding(KeyAction::cancel, "interrupt, send queued messages, or clear the composer");
  binding(KeyAction::edit_queued, "edit the last queued message");
  binding(KeyAction::paste, "paste into the composer");
  binding(KeyAction::external_editor, "open the configured editor");
  binding(KeyAction::scroll_up, "scroll the transcript up");
  binding(KeyAction::scroll_down, "scroll the transcript down");
  binding(KeyAction::toggle_last, "toggle the latest transcript card");
  binding(KeyAction::toggle_all, "expand or collapse every transcript card");
  binding(KeyAction::quit, "quit");
  out += "\nApproval prompt:\n";
  binding(KeyAction::allow_once, "allow once");
  binding(KeyAction::allow_session, "allow for this session");
  binding(KeyAction::allow_project, "save a project grant when available");
  binding(KeyAction::deny, "deny");
  out += "\n!command  run a shell command and include its output in the next model turn\n";
  out += "!!command run a shell command without sending its output to the model\n";
  out += "\nType @ to add a workspace file. Drag to copy. /copy copies the last reply.\n";
  return out;
}

std::vector<Suggestion>
slash_suggestions(const std::string& draft, const std::filesystem::path& dir,
                  const std::string& workspace, std::string_view provider, std::string_view model,
                  const std::vector<std::string>& recents,
                  const std::vector<ExtensionCommand>& extension_commands, const Session& session) {
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

  if ((cmd == "/resume" || cmd == "/restore") && (trailing || !arg.empty())) {
    std::vector<Suggestion> out;
    try {
      for (const auto& info :
           cmd == "/resume" ? list_sessions(dir, workspace) : list_deleted_sessions(dir)) {
        if (!arg.empty() && !session_matches_info(info, arg)) {
          continue;
        }
        out.push_back({cmd + " " + info.id, info.id + "  " + session_title(info)});
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

  if (cmd == "/fork" && (trailing || !arg.empty())) {
    auto space = arg.find(' ');
    if (space != std::string::npos) {
      auto first = arg.substr(0, space);
      if (!first.empty() && std::all_of(first.begin(), first.end(),
                                        [](unsigned char c) { return std::isdigit(c) != 0; })) {
        return {};
      }
    }
    std::vector<Suggestion> out;
    for (const auto& [turn, preview] : session.user_turn_previews()) {
      auto num = std::to_string(turn);
      if (!arg.empty() && !contains_ci(num, arg) && !contains_ci(preview, arg)) {
        continue;
      }
      out.push_back({"/fork " + num, num + "  " + (preview.empty() ? "(empty)" : preview)});
      if (out.size() == 8) {
        break;
      }
    }
    if (!out.empty()) {
      return out;
    }
  }

  if (cmd == "/model" && (provider == "local" || provider == "foundry") &&
      (trailing || !arg.empty())) {
    std::vector<Suggestion> out;
    try {
      for (const auto& entry : models_for(provider)) {
        if (!arg.empty() && !contains_ci(entry.name, arg)) {
          continue;
        }
        auto label = entry.name;
        if (provider == "local") {
          label += "  " + entry.runtime + "  " + format_context_k(entry.context_window);
        } else {
          label += "  " + entry.model;
        }
        out.push_back({"/model " + entry.name, std::move(label)});
      }
    } catch (...) {
    }
    return out;
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
      if (arg.empty() || level.starts_with(arg)) {
        out.push_back({"/thinking " + level, level});
      }
    }
    if (!out.empty()) {
      return out;
    }
  }

  if (cmd == "/theme" && (trailing || !arg.empty())) {
    std::vector<Suggestion> out;
    const auto query = niminal::lower_copy(arg);
    for (const auto& name : theme_names()) {
      if (arg.empty() || name.starts_with(query)) {
        out.push_back({"/theme " + name, name});
      }
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
    for (const auto& name : provider_names()) {
      if (!arg.empty() && !name.starts_with(arg)) {
        continue;
      }
      out.push_back({"/provider " + name, name + "  " + provider_default_model(name)});
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
  auto add_named = [&](std::string slash, std::string description) {
    if (!niminal::lower_copy(slash).starts_with(cmd)) {
      return;
    }
    out.push_back({std::move(slash) + " ", std::move(description)});
  };
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
    add_named(std::move(slash),
              "/" + prompt.name +
                  (prompt.description.empty() ? std::string() : "  " + prompt.description));
  }
  for (const auto& command : extension_commands) {
    auto slash = "/" + command.name;
    if (is_builtin_slash(niminal::lower_copy(slash))) {
      continue;
    }
    add_named(std::move(slash),
              "/" + command.name +
                  (command.description.empty() ? std::string() : "  " + command.description));
  }
  sort_suggestions(out);
  return out;
}

std::optional<std::string> skill_slash_error(const std::filesystem::path& cwd,
                                             std::string_view cmd) {
  if (!cmd.starts_with("/skill:")) {
    return std::nullopt;
  }
  const auto name = cmd.substr(7);
  const auto skills = discover_skills(cwd);
  if (name.empty() || std::none_of(skills.begin(), skills.end(),
                                   [&](const Skill& skill) { return skill.name == name; })) {
    return "unknown skill " + std::string(name);
  }
  return std::nullopt;
}

std::optional<std::string> resolve_prompt_template(const std::filesystem::path& cwd,
                                                   const std::string& prompt, std::string_view cmd,
                                                   std::string_view arg) {
  if (is_builtin_slash(cmd)) {
    return std::nullopt;
  }
  if (auto template_prompt = load_prompt(cwd, std::string(cmd).substr(1))) {
    auto expanded = expand_prompt(cwd, template_prompt->name, std::string(arg));
    return expanded.empty() ? prompt : expanded;
  }
  return std::nullopt;
}

std::string init_prompt(const std::filesystem::path& workspace) {
  const bool claude_only = !std::filesystem::exists(workspace / "AGENTS.md") &&
                           std::filesystem::exists(workspace / "CLAUDE.md");
  const std::string target = claude_only ? "CLAUDE.md" : "AGENTS.md";
  std::string note = claude_only ? std::string(kClaudeInPlaceNote) : std::string();
  if (std::filesystem::exists(workspace / "AGENTS.override.md")) {
    note += kOverrideNote;
  }
  return std::string(kInitPromptHead) + target + "\n\n" + note + std::string(kInitPromptBody);
}

bool is_extension_slash(const std::shared_ptr<ExtensionRuntime>& extensions, std::string_view cmd) {
  if (!extensions) {
    return false;
  }
  for (const auto& command : extensions->commands()) {
    if (niminal::lower_copy("/" + command.name) == cmd) {
      return true;
    }
  }
  return false;
}

bool is_extension_slash_while_busy(const std::shared_ptr<ExtensionRuntime>& extensions,
                                   std::string_view cmd) {
  if (!extensions) {
    return false;
  }
  for (const auto& command : extensions->commands()) {
    if (command.while_busy && niminal::lower_copy("/" + command.name) == cmd) {
      return true;
    }
  }
  return false;
}

} // namespace niminal::app
