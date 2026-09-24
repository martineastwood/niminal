#include "slash_execute.hpp"

#include "clipboard.hpp"
#include "compaction.hpp"
#include "config.hpp"
#include "extensions.hpp"
#include "keybindings.hpp"
#include "models_dev.hpp"
#include "permissions.hpp"
#include "prompts.hpp"
#include "provider.hpp"
#include "session.hpp"
#include "skills.hpp"
#include "slash.hpp"
#include "theme.hpp"
#include "thinking.hpp"
#include "trust.hpp"

#include <niminal/agent.hpp>
#include <niminal/text.hpp>
#include <niminal/version.hpp>

#include <algorithm>
#include <cctype>
#include <fstream>
#include <stdexcept>

namespace niminal::app {
namespace fs = std::filesystem;
using json = nlohmann::json;

namespace {

void push_status(SlashHost& host, std::string text) {
  host.blocks.push_back(Block{BlockKind::status, std::move(text)});
}

void push_error(SlashHost& host, std::string text) {
  host.blocks.push_back(Block{BlockKind::error, std::move(text)});
}

void reject_busy(SlashHost& host) {
  push_status(host, busy_wait_message(host.keybindings));
}

bool handle_quit(SlashHost& host, const std::string&) {
  host.exit_ui();
  return true;
}

bool handle_version(SlashHost& host, const std::string& arg) {
  if (!arg.empty()) {
    push_error(host, "/version takes no arguments");
    return true;
  }
  push_status(host, niminal::version_string());
  return true;
}

bool handle_copy(SlashHost& host, const std::string& arg) {
  if (!arg.empty()) {
    push_error(host, "/copy takes no arguments");
    return true;
  }
  std::string text;
  for (auto it = host.blocks.rbegin(); it != host.blocks.rend(); ++it) {
    if ((it->kind == BlockKind::error || it->kind == BlockKind::assistant) && !it->text.empty()) {
      text = it->text;
      break;
    }
  }
  if (text.empty()) {
    text = host.session.last_assistant_text();
  }
  if (text.empty()) {
    push_status(host, "Nothing to copy yet.");
    return true;
  }
  copy_to_clipboard(text);
  host.flash_footer("Copied to clipboard.");
  push_status(host, "Copied to clipboard.");
  return true;
}

bool handle_yolo(SlashHost& host, const std::string& arg) {
  if (arg.empty() || arg == "on") {
    host.yolo_mode = true;
  } else if (arg == "off") {
    host.yolo_mode = false;
  } else {
    push_error(host, "Usage: /yolo [off]");
  }
  return true;
}

bool handle_retry(SlashHost& host, const std::string& arg) {
  if (!arg.empty()) {
    push_error(host, "/retry takes no arguments");
  } else if (host.busy) {
    reject_busy(host);
  } else if (!host.retry_available) {
    push_status(host, "Nothing to retry.");
  } else {
    host.retry_available = false;
    host.send_prompt(host.retry_prompt, true);
  }
  return true;
}

bool handle_compact(SlashHost& host, const std::string& arg) {
  if (host.busy) {
    reject_busy(host);
    return true;
  }
  try {
    auto result = compact_session(host.session, host.agent, arg, host.extensions, host.cfg);
    host.agent.messages = host.session.openai_messages();
    for (const auto& warning : result.warnings) {
      push_status(host, warning);
    }
    push_status(host, result.message);
  } catch (const std::exception& e) {
    push_error(host, e.what());
  }
  return true;
}

bool handle_help(SlashHost& host, const std::string&) {
  push_status(host, slash_help(host.keybindings));
  return true;
}

bool handle_extension(SlashHost& host, const std::string& cmd, const std::string& arg) {
  if (!host.extensions) {
    push_error(host, "extensions unavailable");
    return true;
  }
  try {
    json context = {{"mode", "tui"},
                    {"workspace", host.cwd.string()},
                    {"session_id", host.session.id},
                    {"provider", host.agent.provider},
                    {"model", host.agent.model},
                    {"messages", host.session.openai_messages()}};
    auto response = host.extensions->invoke(cmd.substr(1), arg, context);
    auto message = response.value("message", std::string());
    if (!message.empty()) {
      push_status(host, std::move(message));
    }
    host.apply_extension_actions();
    bool restarted = false;
    if (auto action = response.find("session"); action != response.end() && action->is_object()) {
      const auto kind = action->value("action", std::string());
      if (kind == "new") {
        if (!host.allow_session_switch("new", "")) {
          return true;
        }
        auto next = create_session(default_session_dir(), host.cwd.string());
        next.persist = host.session.persist;
        if (!next.persist) {
          next.path.clear();
        }
        host.adopt_session(std::move(next), "New session", "new");
        restarted = true;
      } else if (kind == "switch") {
        const auto id = action->value("id", std::string());
        if (!host.allow_session_switch("resume", id)) {
          return true;
        }
        auto next = load_session(default_session_dir(), id);
        next.recover_interrupted_tools();
        host.adopt_session(std::move(next), "Resumed " + id, "resume");
        restarted = true;
      } else if (kind == "compact") {
        const auto compacted =
            compact_session(host.session, host.agent, action->value("instruction", std::string()),
                            host.extensions, host.cfg);
        host.agent.messages = host.session.openai_messages();
        push_status(host, compacted.message);
      }
      const auto editor_text = action->value("editor_text", std::string());
      if (!editor_text.empty()) {
        host.set_draft(editor_text);
      }
    }
    if (response.value("reload", false) && !restarted) {
      host.restart_extensions();
    }
    const auto next_prompt = response.value("prompt", std::string());
    if (!next_prompt.empty()) {
      host.send_prompt(niminal::UserInput{next_prompt}, false);
    }
  } catch (const std::exception& e) {
    push_error(host, e.what());
  }
  return true;
}

void reload_project_resources(SlashHost& host) {
  host.permissions.reload_project();
  refresh_skill_tool(host.agent, host.cwd);
  host.restart_extensions();
  host.reload_local();
}

bool handle_reload(SlashHost& host, const std::string& arg) {
  if (!arg.empty()) {
    push_error(host, "/reload takes no arguments");
    return true;
  }
  reload_project_resources(host);
  push_status(host, "Reloaded project resources.");
  return true;
}

bool handle_permissions(SlashHost& host, const std::string& arg) {
  if (arg.empty()) {
    push_status(host, host.permissions.describe());
  } else if (arg == "clear") {
    try {
      host.permissions.clear_project();
      push_status(host, "Cleared project permission grants.");
    } catch (const std::exception& e) {
      push_error(host, e.what());
    }
  } else {
    push_error(host, "Usage: /permissions [clear]");
  }
  return true;
}

bool handle_trust(SlashHost& host, const std::string& arg) {
  const auto resources = project_trust_resources(host.cwd);
  if (resources.empty()) {
    push_status(host, "No project-local resources require trust.");
  } else if (arg.empty()) {
    push_status(host, std::string("Project-local resources: ") +
                          (project_resources_trusted(host.cwd) ? "trusted" : "not trusted"));
  } else if (arg == "on" || arg == "off") {
    const bool trusted = arg == "on";
    set_project_resources_trusted(host.cwd, trusted);
    try {
      save_project_trust(host.cwd, trusted);
      reload_project_resources(host);
      push_status(host, trusted ? "Project-local resources enabled."
                                : "Project-local resources disabled.");
    } catch (const std::exception& e) {
      push_error(host, e.what());
    }
  } else {
    push_error(host, "Usage: /trust [on|off]");
  }
  return true;
}

bool handle_provider(SlashHost& host, const std::string& arg) {
  if (arg.empty()) {
    push_status(host, "provider: " + host.agent.provider + "\nmodel: " + host.agent.model +
                          "\nurl: " + host.agent.api_url + "\nkey: " + host.agent.key_hint);
    return true;
  }
  if (auto result = select_provider(host.cfg, arg); !result) {
    push_error(host, result.error().what());
    return true;
  }
  apply_provider(host.agent, host.cfg);
  try {
    save_config(host.cfg);
    host.session.add_selection(host.cfg.model, host.agent.provider);
    push_status(host, "provider set to " + host.agent.provider + "\nmodel: " + host.agent.model +
                          "\nsaved " + config_path().string());
  } catch (const std::exception& e) {
    push_error(host, "provider set for this session, save failed: " + std::string(e.what()));
  }
  return true;
}

bool handle_model(SlashHost& host, const std::string& arg) {
  if (arg.empty()) {
    push_status(host, "model: " + host.cfg.model + "\nurl: " + host.agent.api_url);
    return true;
  }
  if (host.cfg.provider == "local" || host.cfg.provider == "foundry") {
    try {
      const auto models = models_for(host.cfg.provider);
      const auto selected =
          std::find_if(models.begin(), models.end(),
                       [&](const ConfiguredModel& model) { return model.name == arg; });
      if (selected == models.end()) {
        push_error(host, "Unknown " + host.cfg.provider + " model '" + arg + "' in " +
                             models_path().string());
        return true;
      }
      host.cfg.api_url = selected->api_url;
    } catch (const std::exception& e) {
      push_error(host, e.what());
      return true;
    }
  }
  host.cfg.model = arg;
  host.cfg.last_models[host.cfg.provider] = arg;
  try {
    apply_provider(host.agent, host.cfg);
  } catch (const std::exception& e) {
    push_error(host, e.what());
    return true;
  }
  try {
    save_config(host.cfg);
    host.session.add_selection(host.cfg.model, host.agent.provider);
    push_status(host, "model set to " + host.cfg.model + "\nsaved " + config_path().string());
  } catch (const std::exception& e) {
    push_error(host, "model set for this session, save failed: " + std::string(e.what()));
  }
  return true;
}

bool handle_thinking(SlashHost& host, const std::string& arg) {
  if (arg.empty()) {
    const auto status = thinking_status(host.agent.provider, host.agent.model, host.cfg.thinking);
    const auto choices = thinking_choices(host.agent.provider, host.agent.model);
    std::string msg = "thinking: ";
    if (host.cfg.thinking.empty()) {
      msg += status.empty() ? "(provider default)" : status;
    } else {
      msg += status.empty() ? host.cfg.thinking : status;
    }
    if (!choices.empty()) {
      msg += "\nlevels: ";
      for (size_t i = 0; i < choices.size(); ++i) {
        if (i != 0) {
          msg += '|';
        }
        msg += choices[i];
      }
    }
    push_status(host, std::move(msg));
    return true;
  }
  try {
    host.cfg.thinking = normalize_thinking(arg);
  } catch (const std::exception& e) {
    push_error(host, e.what());
    return true;
  }
  apply_provider(host.agent, host.cfg);
  const auto status = thinking_status(host.agent.provider, host.agent.model, host.cfg.thinking);
  try {
    save_config(host.cfg);
    push_status(host, "thinking set to " + (status.empty() ? host.cfg.thinking : status) +
                          "\nsaved " + config_path().string());
  } catch (const std::exception& e) {
    push_error(host, "thinking set for this session, save failed: " + std::string(e.what()));
  }
  return true;
}

bool handle_theme(SlashHost& host, const std::string& arg) {
  if (arg.empty()) {
    push_status(host, std::string("theme: ") + host.cfg.theme +
                          (host.cfg.theme == "auto"
                               ? std::string(" (") + theme_mode_name(detect_terminal_theme()) + ")"
                               : std::string()));
    return true;
  }
  const auto selected = load_theme(arg);
  if (!selected) {
    push_error(host, selected.error());
    return true;
  }
  host.theme = *selected;
  host.cfg.theme = arg;
  try {
    save_config(host.cfg);
    push_status(host, std::string("theme set to ") + host.cfg.theme + "\nsaved " +
                          config_path().string());
  } catch (const std::exception& e) {
    push_error(host, "theme set for this session, save failed: " + std::string(e.what()));
  }
  return true;
}

bool handle_settings(SlashHost& host, const std::string& arg) {
  if (!arg.empty()) {
    push_error(host, "Usage: /settings");
    return true;
  }
  host.settings_open = true;
  host.settings_i = 0;
  host.settings_edit = std::nullopt;
  host.settings_error.clear();
  return true;
}

bool handle_models(SlashHost& host, const std::string& arg) {
  if ((host.cfg.provider == "local" || host.cfg.provider == "foundry") && arg.empty()) {
    try {
      const auto models = models_for(host.cfg.provider);
      std::string list = host.cfg.provider + " models (" + models_path().string() + "):";
      for (const auto& model : models) {
        list +=
            "\n" + model.name + "  " + (host.cfg.provider == "local" ? model.runtime : model.model);
        if (model.context_window > 0) {
          list += "  " + format_context_k(model.context_window);
        }
        if (model.name == host.cfg.model) {
          list += "  (active)";
        }
      }
      push_status(host, std::move(list));
    } catch (const std::exception& e) {
      push_error(host, e.what());
    }
    return true;
  }
  if (arg != "refresh") {
    push_error(host, host.cfg.provider == "local" || host.cfg.provider == "foundry"
                         ? "Use /model NAME to select a configured model"
                         : "Usage: /models refresh");
    return true;
  }
  if (refresh_catalog()) {
    push_status(host, "Updated model catalog  ·  " + catalog_cache_path().string());
  } else {
    push_error(host, "Could not refresh model metadata; using existing cache.");
  }
  return true;
}

bool handle_session(SlashHost& host, const std::string&) {
  push_status(host, host.session.describe());
  return true;
}

bool handle_name(SlashHost& host, const std::string& arg) {
  if (arg.empty()) {
    push_status(host, host.session.name.empty() ? "Name: (none)" : "Name: " + host.session.name);
    return true;
  }
  host.session.add_name(arg);
  push_status(host, "Name: " + host.session.name);
  return true;
}

bool handle_resume(SlashHost& host, const std::string& arg) {
  const auto dir = default_session_dir();
  if (arg.empty() || !valid_session_id(arg)) {
    const auto infos = search_sessions(dir, host.cwd.string(), arg);
    push_status(host, format_session_list(infos, host.session.id));
    return true;
  }
  try {
    if (!host.allow_session_switch("resume", arg)) {
      return true;
    }
    auto next = load_session(dir, arg);
    next.recover_interrupted_tools();
    host.adopt_session(std::move(next), "Resumed " + arg, "resume");
  } catch (const std::exception& e) {
    push_error(host, e.what());
  }
  return true;
}

bool handle_search(SlashHost& host, const std::string& arg) {
  if (arg.empty()) {
    push_error(host, "Usage: /search TEXT");
    return true;
  }
  const auto infos = search_sessions(default_session_dir(), host.cwd.string(), arg);
  push_status(host, format_session_list(infos, host.session.id, "Matches for " + arg));
  return true;
}

bool handle_fork(SlashHost& host, const std::string& arg) {
  try {
    int upto = -1;
    int turn = 0;
    std::string title;
    if (!arg.empty()) {
      const auto space = arg.find(' ');
      const std::string first =
          niminal::trim_copy(space == std::string::npos ? arg : arg.substr(0, space));
      const bool numeric =
          !first.empty() && std::all_of(first.begin(), first.end(),
                                        [](const unsigned char c) { return std::isdigit(c) != 0; });
      if (numeric) {
        turn = std::stoi(first);
        if (space != std::string::npos) {
          title = niminal::trim_copy(arg.substr(space + 1));
        }
        upto = host.session.end_after_user_turn(turn);
        if (upto < 0) {
          push_error(host, "No user turn " + std::to_string(turn));
          return true;
        }
      } else {
        title = arg;
      }
    }
    if (!host.allow_session_switch("fork", host.session.id)) {
      return true;
    }
    auto next = upto < 0 ? host.session.fork(default_session_dir())
                         : host.session.fork(default_session_dir(), upto);
    if (!title.empty()) {
      next.add_name(title);
    }
    const auto id = next.id;
    std::string note = "Forked " + id;
    if (turn > 0) {
      note += " from turn " + std::to_string(turn);
    }
    host.adopt_session(std::move(next), note, "fork");
  } catch (const std::exception& e) {
    push_error(host, e.what());
  }
  return true;
}

bool handle_export(SlashHost& host, const std::string& arg) {
  const auto path = arg.empty() ? host.cwd / (host.session.id + ".md") : fs::path(arg);
  try {
    if (path.has_parent_path()) {
      fs::create_directories(path.parent_path());
    }
    std::ofstream out(path, std::ios::binary | std::ios::trunc);
    if (!out) {
      throw std::runtime_error("cannot write " + path.string());
    }
    const auto ext = path.extension().string();
    std::string format = "md";
    if (ext == ".json") {
      format = "json";
    } else if (ext == ".html" || ext == ".htm") {
      format = "html";
    }
    out << host.session.export_text(format);
    push_status(host, "Exported " + std::to_string(host.session.events.size()) + " events to " +
                          path.string());
  } catch (const std::exception& e) {
    push_error(host, e.what());
  }
  return true;
}

bool handle_delete(SlashHost& host, const std::string& arg) {
  if (arg.empty()) {
    push_error(host, "Usage: /delete ID  ·  /restore lists deleted sessions");
  } else if (arg == host.session.id) {
    push_error(host, "Cannot delete the current session.");
  } else if (delete_session(default_session_dir(), arg)) {
    push_status(host, "Deleted " + arg + "  ·  /restore " + arg + " brings it back");
  } else {
    push_error(host, "No session " + arg);
  }
  return true;
}

bool handle_restore(SlashHost& host, const std::string& arg) {
  const auto dir = default_session_dir();
  if (arg.empty() || !valid_session_id(arg)) {
    const auto infos = list_deleted_sessions(dir);
    push_status(host,
                format_session_list(infos, host.session.id, "Deleted sessions (newest first)"));
  } else if (restore_session(dir, arg)) {
    push_status(host, "Restored " + arg + "  ·  /resume " + arg);
  } else {
    push_error(host, "No deleted session " + arg);
  }
  return true;
}

bool handle_new(SlashHost& host, const std::string&) {
  if (!host.allow_session_switch("new", "")) {
    return true;
  }
  auto next = create_session(default_session_dir(), host.cwd.string());
  next.persist = host.session.persist;
  if (!next.persist) {
    next.path.clear();
  }
  const auto id = next.id;
  host.adopt_session(std::move(next), "New session " + id, "new");
  apply_provider(host.agent, host.cfg);
  return true;
}

} // namespace

bool execute_slash(SlashHost& host, const std::string& cmd, const std::string& arg,
                   bool extension_request) {
  if (cmd == "/quit" || cmd == "/exit") {
    return handle_quit(host, arg);
  }
  if (cmd == "/version") {
    return handle_version(host, arg);
  }
  if (cmd == "/copy") {
    return handle_copy(host, arg);
  }
  if (cmd == "/yolo") {
    return handle_yolo(host, arg);
  }
  if (cmd == "/retry") {
    return handle_retry(host, arg);
  }
  if (cmd == "/compact") {
    return handle_compact(host, arg);
  }
  if (cmd == "/help") {
    return handle_help(host, arg);
  }
  if (!extension_request && cmd == "/theme") {
    return handle_theme(host, arg);
  }
  if (!extension_request && cmd == "/models" && arg.empty()) {
    return handle_models(host, arg);
  }
  if (!extension_request && cmd == "/provider" && arg.empty()) {
    return handle_provider(host, arg);
  }
  if (!extension_request && cmd == "/model" && arg.empty()) {
    return handle_model(host, arg);
  }
  if (!extension_request && cmd == "/thinking" && arg.empty()) {
    return handle_thinking(host, arg);
  }
  if (!extension_request && cmd == "/search") {
    return handle_search(host, arg);
  }
  if (host.busy) {
    if (!extension_request && !arg.empty() &&
        (cmd == "/provider" || cmd == "/model" || cmd == "/thinking")) {
      host.pending_changes.emplace_back(cmd, arg);
      push_status(host, cmd + " " + arg + " will apply after this turn.");
      return true;
    }
    reject_busy(host);
    return true;
  }
  if (extension_request) {
    return handle_extension(host, cmd, arg);
  }
  if (cmd == "/reload") {
    return handle_reload(host, arg);
  }
  if (cmd == "/permissions") {
    return handle_permissions(host, arg);
  }
  if (cmd == "/trust") {
    return handle_trust(host, arg);
  }
  if (cmd == "/provider") {
    return handle_provider(host, arg);
  }
  if (cmd == "/model") {
    return handle_model(host, arg);
  }
  if (cmd == "/thinking") {
    return handle_thinking(host, arg);
  }
  if (cmd == "/theme") {
    return handle_theme(host, arg);
  }
  if (cmd == "/settings") {
    return handle_settings(host, arg);
  }
  if (cmd == "/models") {
    return handle_models(host, arg);
  }
  if (cmd == "/session") {
    return handle_session(host, arg);
  }
  if (cmd == "/name") {
    return handle_name(host, arg);
  }
  if (cmd == "/resume") {
    return handle_resume(host, arg);
  }
  if (cmd == "/search") {
    return handle_search(host, arg);
  }
  if (cmd == "/fork") {
    return handle_fork(host, arg);
  }
  if (cmd == "/export") {
    return handle_export(host, arg);
  }
  if (cmd == "/delete") {
    return handle_delete(host, arg);
  }
  if (cmd == "/restore") {
    return handle_restore(host, arg);
  }
  if (cmd == "/clear" || cmd == "/new") {
    return handle_new(host, arg);
  }
  push_error(host, "unknown command " + cmd + "  ·  /help");
  return true;
}

} // namespace niminal::app
