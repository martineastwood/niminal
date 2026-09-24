#include "tui.hpp"
#include "clipboard.hpp"
#include "compaction.hpp"
#include "config.hpp"
#include "diff.hpp"
#include "keybindings.hpp"
#include "markdown.hpp"
#include "mentions.hpp"
#include "models_dev.hpp"
#include "permissions.hpp"
#include "prompts.hpp"
#include "provider.hpp"
#include "session.hpp"
#include "settings.hpp"
#include "skills.hpp"
#include "slash.hpp"
#include "slash_execute.hpp"
#include "theme.hpp"
#include "thinking.hpp"
#include "tools.hpp"
#include "transcript.hpp"
#include "trust.hpp"

#include <niminal/openai.hpp>
#include <niminal/text.hpp>
#include <niminal/version.hpp>

#include <ftxui/component/component.hpp>
#include <ftxui/component/mouse.hpp>
#include <ftxui/component/screen_interactive.hpp>
#include <ftxui/dom/elements.hpp>
#include <ftxui/screen/string.hpp>

#include <algorithm>
#include <atomic>
#include <cctype>
#include <chrono>
#include <condition_variable>
#include <exception>
#include <filesystem>
#include <fstream>
#include <future>
#include <iostream>
#include <mutex>
#include <optional>
#include <sstream>
#include <termios.h>
#include <thread>
#include <tuple>
#include <unistd.h>
#include <unordered_map>
#include <utility>
#include <vector>

namespace niminal::app {
namespace {

using namespace ftxui;

struct PendingFileChange {
  std::filesystem::path path;
  std::string relative;
  bool before_exists = false;
  std::string before;
  std::string tool_name;
  json input;
};

std::optional<std::string> read_text_file(const std::filesystem::path& path) {
  std::error_code ec;
  if (!std::filesystem::is_regular_file(path, ec) ||
      std::filesystem::file_size(path, ec) > 200'000) {
    return std::nullopt;
  }
  std::ifstream in(path, std::ios::binary);
  if (!in) {
    return std::nullopt;
  }
  std::ostringstream out;
  out << in.rdbuf();
  auto text = out.str();
  if (text.find('\0') != std::string::npos) {
    return std::nullopt;
  }
  return text;
}

bool is_wheel_up(Event e) {
  return e.is_mouse() && e.mouse().button == Mouse::WheelUp;
}

bool is_wheel_down(Event e) {
  return e.is_mouse() && e.mouse().button == Mouse::WheelDown;
}

void add_unique(std::vector<std::string>& ids, const std::string& id) {
  if (id.empty()) {
    return;
  }
  for (const auto& x : ids) {
    if (x == id) {
      return;
    }
  }
  ids.push_back(id);
}

bool is_word_byte(char c) {
  const auto b = static_cast<unsigned char>(c);
  return (b >= '0' && b <= '9') || (b >= 'a' && b <= 'z') || (b >= 'A' && b <= 'Z') || b == '_';
}

// The cursor only ever lands on glyph boundaries: bytes >= 0x80 are never word
// bytes, so both walks skip whole multi-byte characters.
int cursor_word_left(const std::string& text, int pos) {
  size_t i = std::min(static_cast<size_t>(std::max(0, pos)), text.size());
  while (i > 0 && !is_word_byte(text[i - 1])) {
    --i;
  }
  while (i > 0 && is_word_byte(text[i - 1])) {
    --i;
  }
  return static_cast<int>(i);
}

int cursor_word_right(const std::string& text, int pos) {
  size_t i = std::min(static_cast<size_t>(std::max(0, pos)), text.size());
  while (i < text.size() && !is_word_byte(text[i])) {
    ++i;
  }
  while (i < text.size() && is_word_byte(text[i])) {
    ++i;
  }
  return static_cast<int>(i);
}

int cursor_glyph_left(const std::string& text, int pos) {
  size_t i = std::min(static_cast<size_t>(std::max(0, pos)), text.size());
  if (i > 0) {
    --i;
    while (i > 0 && (static_cast<unsigned char>(text[i]) & 0xC0) == 0x80) {
      --i;
    }
  }
  return static_cast<int>(i);
}

int cursor_glyph_right(const std::string& text, int pos) {
  size_t i = std::min(static_cast<size_t>(std::max(0, pos)), text.size());
  if (i < text.size()) {
    ++i;
    while (i < text.size() && (static_cast<unsigned char>(text[i]) & 0xC0) == 0x80) {
      ++i;
    }
  }
  return static_cast<int>(i);
}

constexpr size_t kQueuePreviewLineLimit = 3;

std::vector<std::string> preview_message_lines(const std::string& message) {
  std::vector<std::string> lines;
  size_t start = 0;
  while (start < message.size() && lines.size() < kQueuePreviewLineLimit) {
    const auto end = message.find('\n', start);
    if (end == std::string::npos) {
      lines.push_back(message.substr(start));
      break;
    }
    lines.push_back(message.substr(start, end - start));
    start = end + 1;
  }
  if (start < message.size() && !lines.empty()) {
    lines.back() += " …";
  }
  return lines;
}

std::string compose_input_preview(const niminal::UserInput& input) {
  std::string text = input.text;
  for (const auto& image : input.images) {
    if (!text.empty()) {
      text.push_back('\n');
    }
    text += "[image: ";
    text += image.value("name", "image");
    text.push_back(']');
  }
  return text;
}

Elements render_queue_preview(const std::vector<std::string>& steering,
                              const std::vector<std::string>& follow_up,
                              const Keybindings& keybindings) {
  if (steering.empty() && follow_up.empty()) {
    return {};
  }
  Elements rows;
  if (!steering.empty()) {
    rows.push_back(
        text("After next model step (" + keybindings.label(KeyAction::cancel) + " to send now)") |
        dim);
    for (const auto& message : steering) {
      for (const auto& line : preview_message_lines(message)) {
        rows.push_back(text(" ↳ " + line) | dim);
      }
    }
  }
  if (!follow_up.empty()) {
    if (!steering.empty()) {
      rows.push_back(text(""));
    }
    rows.push_back(text("After this turn") | dim);
    for (const auto& message : follow_up) {
      for (const auto& line : preview_message_lines(message)) {
        rows.push_back(text(" ↳ " + line) | dim);
      }
    }
  }
  if (!steering.empty()) {
    rows.push_back(text(keybindings.label(KeyAction::edit_queued) + " edit last") | dim);
  }
  return rows;
}

ssize_t last_card_index(const std::vector<Block>& blocks) {
  for (ssize_t i = static_cast<ssize_t>(blocks.size()) - 1; i >= 0; --i) {
    if (is_card_block(blocks[static_cast<size_t>(i)].kind)) {
      return i;
    }
  }
  return -1;
}

std::optional<size_t> card_at(const std::vector<Block>& blocks, const std::vector<Box>& boxes,
                              int x, int y) {
  const auto count = std::min(blocks.size(), boxes.size());
  for (size_t i = 0; i < count; ++i) {
    if (!is_card_block(blocks[i].kind)) {
      continue;
    }
    const auto& box = boxes[i];
    if (box.x_min <= x && x <= box.x_max && box.y_min <= y && y <= box.y_max) {
      return i;
    }
  }
  return std::nullopt;
}

} // namespace

int run_tui(niminal::Agent& agent, Workspace& workspace, Config& cfg, Session& session,
            std::shared_ptr<ExtensionRuntime>& extensions, bool yolo,
            const std::vector<std::string>* allowed_tools,
            std::function<void()> reload_system_prompt) {
  const auto& cwd = workspace.root();
  auto screen = ScreenInteractive::Fullscreen();
  Theme theme = load_theme(cfg.theme).value_or(resolve_theme(ThemeMode::automatic));
  std::vector<std::pair<std::string, Element>> markdown_cache;
  std::atomic<bool> local_cancel{false};
  if (agent.cancel == nullptr) {
    agent.cancel = &local_cancel;
  }
  auto* cancel = agent.cancel;

  auto loaded_keybindings = load_keybindings();
  const auto& keybindings = loaded_keybindings.bindings;
  std::vector<Block> blocks;
  std::mutex file_changes_mu;
  std::unordered_map<std::string, PendingFileChange> file_changes;
  size_t step_block_start = 0;

  std::string draft;
  json draft_images = json::array();
  int cursor = 0;
  std::vector<niminal::UserInput> history;
  int history_i = -1;
  niminal::UserInput live_draft;
  std::mutex steering_mu;
  std::vector<niminal::UserInput> steering;
  std::vector<niminal::UserInput> follow_up;
  std::vector<std::pair<std::string, std::string>> pending_changes;
  std::vector<std::string> idle_extension_messages;
  std::function<void(std::string)> deliver_extension_now;
  std::function<void()> handle_turn_idle;
  std::function<void()> apply_pending_changes;
  bool send_queue_after_stop = false;
  bool plain_interrupt_pending = false;
  std::vector<ExtensionEntry> extension_entries_pending;
  std::atomic<bool> busy{false};
  std::atomic<bool> user_bash_running{false};
  std::string activity;
  std::optional<std::chrono::steady_clock::time_point> activity_started;
  std::string footer_notice;
  std::atomic<bool> footer_notice_active{false};
  niminal::UserInput retry_prompt;
  bool retry_available = false;
  bool turn_failed = false;
  std::chrono::steady_clock::time_point footer_notice_until;
  std::optional<size_t> extension_action_focus;
  float transcript_y = 1.F;
  int transcript_rows = 1;
  bool ask_user_open = false;
  std::string ask_user_question;
  std::vector<std::string> ask_user_options;
  int ask_user_i = 0;
  bool ask_user_editing_other = false;
  std::string ask_user_answer;
  int ask_user_cursor = 0;
  std::function<void(niminal::ToolResult)> ask_user_complete;
  bool pasting = false;
  bool stick_bottom = true;
  std::vector<Box> card_boxes;
  std::optional<size_t> card_press_index;
  std::optional<std::pair<int, int>> mouse_press;
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
    if (!call->done) {
      throw std::runtime_error("interactive UI is unavailable");
    }
    if (call->error) {
      std::rethrow_exception(call->error);
    }
  };

  // Run `body` with the terminal restored, on the UI thread. FTXUI's
  // WithRestoredIO returns a closure rather than calling it, so dropping the
  // returned value silently skips the body; keep the call in one place.
  auto with_restored_io = [&](const std::function<void()>& body) {
    run_on_ui([&] { screen.WithRestoredIO(body)(); });
  };

  auto finish_ask_user = [&](niminal::ToolResult result) {
    if (!ask_user_open || !ask_user_complete) {
      return;
    }
    ask_user_open = false;
    ask_user_editing_other = false;
    auto complete = std::move(ask_user_complete);
    complete(std::move(result));
  };

  const bool ask_user_allowed =
      allowed_tools == nullptr ||
      std::find(allowed_tools->begin(), allowed_tools->end(), "ask_user") != allowed_tools->end();
  if (ask_user_allowed) {
    agent.tools.push_back(niminal::Tool{
        "ask_user",
        "Ask the user a multiple-choice question in the TUI with 2 to 4 concise choices. The "
        "widget adds an Other choice for a free-text answer.",
        json{{"type", "object"},
             {"properties",
              {{"question", {{"type", "string"}, {"description", "Question to ask the user."}}},
               {"options",
                {{"type", "array"},
                 {"items", {{"type", "string"}}},
                 {"description", "2 to 4 concise choices; Other is added automatically."},
                 {"minItems", 2},
                 {"maxItems", 4}}}}},
             {"required", json::array({"question", "options"})},
             {"additionalProperties", false}},
        [&](const json& input) {
          const auto question = input.at("question").get<std::string>();
          const auto options = input.at("options").get<std::vector<std::string>>();
          if (question.empty() || options.size() < 2 || options.size() > 4) {
            return niminal::ToolResult{"tool error: ask_user needs a question and 2 to 4 choices"};
          }
          auto answer = std::make_shared<std::promise<niminal::ToolResult>>();
          auto result = answer->get_future();
          run_on_ui([&, question, options, answer] {
            ask_user_question = question;
            ask_user_options = options;
            ask_user_i = 0;
            ask_user_editing_other = false;
            ask_user_answer.clear();
            ask_user_cursor = 0;
            ask_user_complete = [answer](niminal::ToolResult value) {
              answer->set_value(std::move(value));
            };
            ask_user_open = true;
            screen.RequestAnimationFrame();
          });
          return result.get();
        },
        false});
  }

  auto configure_extension_ui = [&](const std::shared_ptr<ExtensionRuntime>& runtime) {
    if (!runtime) {
      return;
    }
    ExtensionUiCallbacks callbacks;
    callbacks.question = [&](const std::string& prompt, const std::vector<std::string>& options) {
      std::string answer;
      with_restored_io([&] {
        std::cout << "\n" << prompt;
        if (!options.empty()) {
          std::cout << "\n";
          for (size_t i = 0; i < options.size(); ++i) {
            std::cout << "  [" << (i + 1) << "] " << options[i] << "\n";
          }
        }
        std::cout << "> " << std::flush;
        std::getline(std::cin, answer);
        if (!options.empty() && answer.size() == 1 && answer[0] >= '1' &&
            answer[0] <= static_cast<char>('0' + options.size())) {
          answer = options[static_cast<size_t>(answer[0] - '1')];
        }
      });
      return answer;
    };
    callbacks.input = [&](const std::string& prompt, bool secret) {
      std::string answer;
      with_restored_io([&] {
        std::cout << "\n" << prompt << ": " << std::flush;
        termios old_termios{};
        const bool hidden = secret && tcgetattr(STDIN_FILENO, &old_termios) == 0;
        if (hidden) {
          auto hidden_termios = old_termios;
          hidden_termios.c_lflag &= ~static_cast<tcflag_t>(ECHO);
          tcsetattr(STDIN_FILENO, TCSANOW, &hidden_termios);
        }
        std::getline(std::cin, answer);
        if (hidden) {
          tcsetattr(STDIN_FILENO, TCSANOW, &old_termios);
          std::cout << "\n";
        }
      });
      return answer;
    };
    callbacks.editor = [&](const std::string&, const std::string& text) {
      std::string edited;
      with_restored_io([&] { edited = edit_text_externally(text, cfg.editor); });
      return edited;
    };
    runtime->set_ui_callbacks(std::move(callbacks));
  };
  int suggest_i = 0;
  bool settings_open = false;
  int settings_i = 0;
  std::optional<std::string> settings_edit;
  std::string settings_error;
  std::string suggest_sig;
  std::filesystem::path session_dir;
  try {
    session_dir = default_session_dir();
  } catch (...) {
  }

  std::vector<Suggestion> suggestions_cache;
  std::optional<std::tuple<std::string, int, std::string, std::string>> suggestions_input;
  auto current_suggestions = [&]() -> const std::vector<Suggestion>& {
    const auto input_key = std::tuple{draft, cursor, agent.provider, agent.model};
    if (suggestions_input == input_key) {
      return suggestions_cache;
    }

    std::vector<std::string> recents;
    add_unique(recents, agent.model);
    if (auto it = cfg.last_models.find(agent.provider); it != cfg.last_models.end()) {
      add_unique(recents, it->second);
    }
    add_unique(recents, cfg.model);
    std::vector<Suggestion> items;
    if (auto mention = file_mention_at(draft, static_cast<size_t>(cursor))) {
      for (const auto& path : suggest_mentioned_files(workspace, mention->query)) {
        items.push_back({path, "@" + path, true});
      }
    } else {
      static const std::vector<ExtensionCommand> no_commands;
      const auto& commands = extensions ? extensions->commands() : no_commands;
      items = slash_suggestions(draft, session_dir, cwd.string(), agent.provider, agent.model,
                                recents, commands, session);
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
    if (!items.empty()) {
      suggest_i = std::clamp(suggest_i, 0, static_cast<int>(items.size()) - 1);
    }
    suggestions_input = input_key;
    suggestions_cache = std::move(items);
    return suggestions_cache;
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
      if (!event.is_object() || event.value("type", "") != "user") {
        continue;
      }
      std::string text;
      auto images = json::array();
      for (const auto& part : event.value("content", json::array())) {
        if (part.is_object() && part.value("type", "") == "text") {
          text += part.value("text", "");
        } else if (part.is_object() && part.value("type", "") == "image") {
          images.push_back(part);
        }
      }
      if (!text.empty() || !images.empty()) {
        history.emplace_back(std::move(text), std::move(images));
      }
    }
    if (history.size() > 500) {
      history.erase(history.begin(), history.end() - 500);
    }
    history_i = -1;
    live_draft = {};
  };
  load_history();

  auto remember_input = [&](const niminal::UserInput& input) {
    if (input.text.empty() && input.images.empty()) {
      return;
    }
    if (!input.text.empty() && input.text[0] == '/') {
      return;
    }
    if (history.empty() || history.back().text != input.text ||
        history.back().images != input.images) {
      history.push_back(input);
    }
    if (history.size() > 500) {
      history.erase(history.begin(),
                    history.begin() + static_cast<std::ptrdiff_t>(history.size() - 500));
    }
    history_i = -1;
  };

  auto flash_footer = [&](std::string message) {
    footer_notice = std::move(message);
    footer_notice_until = std::chrono::steady_clock::now() + std::chrono::milliseconds(1500);
    footer_notice_active = true;
    screen.RequestAnimationFrame();
  };

  auto persist_settings = [&](const SettingApplyResult& result) {
    if (!result.error.empty()) {
      settings_error = result.error;
      return;
    }
    settings_error.clear();
    suggestions_input.reset();
    try {
      if (result.theme_changed) {
        theme = load_theme(cfg.theme).value();
        markdown_cache.clear();
      }
      if (result.agent_changed) {
        apply_provider(agent, cfg);
        session.add_selection(cfg.model, agent.provider);
      }
      save_config(cfg);
      flash_footer("Saved " + config_path().string());
    } catch (const std::exception& e) {
      settings_error = e.what();
    }
  };

  auto history_prev = [&] {
    if (history.empty()) {
      return;
    }
    if (history_i < 0) {
      live_draft = niminal::UserInput{draft, draft_images};
      history_i = static_cast<int>(history.size()) - 1;
    } else if (history_i > 0) {
      --history_i;
    }
    draft = history[static_cast<size_t>(history_i)].text;
    draft_images = history[static_cast<size_t>(history_i)].images;
    cursor = static_cast<int>(draft.size());
  };

  auto history_next = [&] {
    if (history_i < 0) {
      return;
    }
    if (history_i + 1 < static_cast<int>(history.size())) {
      ++history_i;
      draft = history[static_cast<size_t>(history_i)].text;
      draft_images = history[static_cast<size_t>(history_i)].images;
    } else {
      history_i = -1;
      draft = live_draft.text;
      draft_images = live_draft.images;
    }
    cursor = static_cast<int>(draft.size());
  };

  auto apply_extension_actions = [&] {
    if (!extensions) {
      return;
    }
    extensions->pump();
    for (auto& notice : extensions->take_notices()) {
      blocks.push_back(Block{notice.level == "error" ? BlockKind::error : BlockKind::status,
                             std::move(notice.message)});
    }
    auto entries = extensions->take_entries();
    extension_entries_pending.insert(extension_entries_pending.end(),
                                     std::make_move_iterator(entries.begin()),
                                     std::make_move_iterator(entries.end()));
    if (!busy) {
      for (const auto& entry : extension_entries_pending) {
        session.add_extension(entry.extension, entry.data);
      }
      extension_entries_pending.clear();
    }
    for (auto& message : extensions->take_user_messages()) {
      if (!busy && deliver_extension_now) {
        deliver_extension_now(std::move(message.content));
      } else if (!busy) {
        idle_extension_messages.push_back(std::move(message.content));
      } else {
        std::lock_guard lock(steering_mu);
        if (message.deliver_as == "steer") {
          steering.push_back(message.content);
        } else {
          follow_up.push_back(message.content);
        }
      }
    }
  };

  auto usage_totals = session.usage_totals();
  size_t footer_revision = 0;
  auto apply_event = [&](StreamEvent ev) {
    if (ev.kind == EventKind::tool_result) {
      suggestions_input.reset();
    }
    switch (ev.kind) {
    case EventKind::text_delta:
      if (blocks.empty() || blocks.back().kind != BlockKind::assistant) {
        blocks.push_back(Block{BlockKind::assistant, {}});
      }
      blocks.back().text += ev.text;
      activity = "Responding…";
      break;
    case EventKind::thinking_delta:
      if (blocks.empty() || blocks.back().kind != BlockKind::thinking) {
        Block thinking{BlockKind::thinking, {}};
        thinking.expanded = cfg.show_thinking;
        blocks.push_back(std::move(thinking));
      }
      blocks.back().text += ev.text;
      activity = "Thinking…";
      break;
    case EventKind::tool_output_delta:
      if (!ev.tool_id.empty()) {
        auto tool = std::find_if(blocks.rbegin(), blocks.rend(), [&](const Block& block) {
          return block.kind == BlockKind::tool && block.tool_id == ev.tool_id;
        });
        if (tool != blocks.rend()) {
          tool->result = ev.text;
        }
      }
      break;
    case EventKind::tool_call: {
      Block tool{BlockKind::tool, ev.text};
      tool.tool_name = ev.tool_name;
      tool.tool_id = ev.tool_id;
      blocks.push_back(std::move(tool));
    }
      activity = ev.tool_name.empty() ? "Waiting for model…" : "Running " + ev.tool_name + "…";
      activity_started = std::chrono::steady_clock::now();
      break;
    case EventKind::approval_required:
      blocks.push_back(
          Block{BlockKind::approval, ev.tool_name + "\n  Allow " + ev.tool_name +
                                         (ev.text.empty() ? std::string() : ": " + ev.text) +
                                         "\n  [enter] once  [s] session" +
                                         (ev.can_remember ? "  [p] project" : "") + "  [n] deny"});
      activity = "Approval needed";
      break;
    case EventKind::tool_result:
      if (!ev.tool_id.empty()) {
        auto tool = std::find_if(blocks.rbegin(), blocks.rend(), [&](const Block& block) {
          return block.kind == BlockKind::tool && block.tool_id == ev.tool_id;
        });
        if (tool != blocks.rend()) {
          tool->result = ev.text;
        }
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
          const bool after_exists = std::filesystem::is_regular_file(pending->path, ec);
          auto after = after_exists ? read_text_file(pending->path) : std::optional<std::string>{};
          if (after_exists && after) {
            auto diff = make_tool_diff(pending->tool_name, pending->input,
                                       !pending->before_exists && after_exists, ev.text);
            if (diff.changed) {
              auto body = std::move(diff.body);
              if (tool != blocks.rend()) {
                tool->kind = BlockKind::diff;
                tool->text = std::move(body);
                tool->path = std::move(pending->relative);
                tool->tool_name = std::move(pending->tool_name);
              } else {
                blocks.push_back(Block{BlockKind::diff, std::move(body),
                                       std::move(pending->relative),
                                       std::move(pending->tool_name)});
              }
            }
          }
        }
      }
      activity = "Waiting for model…";
      activity_started.reset();
      break;
    case EventKind::user:
      blocks.push_back(Block{BlockKind::user, ev.text});
      break;
    case EventKind::status:
      if (ev.retry) {
        const auto start = std::min(step_block_start, blocks.size());
        {
          std::lock_guard lock(file_changes_mu);
          for (size_t i = start; i < blocks.size();) {
            const auto kind = blocks[i].kind;
            if (kind == BlockKind::assistant || kind == BlockKind::thinking ||
                kind == BlockKind::tool || kind == BlockKind::diff) {
              if (!blocks[i].tool_id.empty()) {
                file_changes.erase(blocks[i].tool_id);
              }
              blocks.erase(blocks.begin() + static_cast<std::ptrdiff_t>(i));
            } else {
              ++i;
            }
          }
        }
        activity = ev.text;
        break;
      }
      if (!ev.text.empty()) {
        blocks.push_back(Block{BlockKind::status, ev.text});
      }
      break;
    case EventKind::error:
      blocks.push_back(Block{BlockKind::error, ev.text});
      turn_failed = true;
      activity.clear();
      activity_started.reset();
      retry_available =
          ev.text != "interrupted" && (!retry_prompt.text.empty() || !retry_prompt.images.empty());
      break;
    case EventKind::done:
      busy = false;
      activity.clear();
      activity_started.reset();
      if (!turn_failed) {
        retry_available = false;
      }
      turn_failed = false;
      if (apply_pending_changes) {
        apply_pending_changes();
      }
      if (handle_turn_idle) {
        handle_turn_idle();
      }
      break;
    case EventKind::run_start:
      break;
    case EventKind::step_start:
      step_block_start = blocks.size();
      break;
    case EventKind::step_end:
    case EventKind::run_end:
    case EventKind::assistant_message:
      break;
    }
    if (stick_bottom) {
      transcript_y = 1.F;
    }
    if (blocks.size() > 80) {
      blocks.erase(blocks.begin(),
                   blocks.begin() + static_cast<std::ptrdiff_t>(blocks.size() - 80));
    }
  };

  auto post_ui = [&](const StreamEvent& ev) {
    if (ev.kind == EventKind::tool_call && (ev.tool_name == "edit" || ev.tool_name == "write")) {
      try {
        auto path_arg =
            ev.input.is_object() ? ev.input.value("path", std::string()) : std::string();
        if (!path_arg.empty()) {
          auto path = workspace.resolve(path_arg);
          auto relative = workspace.relative(path);
          std::error_code ec;
          const bool before_exists = std::filesystem::is_regular_file(path, ec);
          auto before = before_exists ? read_text_file(path) : std::optional<std::string>{};
          if (!before_exists || before) {
            std::lock_guard lock(file_changes_mu);
            file_changes[ev.tool_id] =
                PendingFileChange{std::move(path), std::move(relative),
                                  before_exists,   before.value_or(std::string()),
                                  ev.tool_name,    ev.input};
          }
        }
      } catch (...) {
      }
    }
    if (!ui_alive) {
      return;
    }
    std::optional<niminal::Usage> updated_usage;
    if (ev.kind == EventKind::step_end || ev.kind == EventKind::done ||
        ev.kind == EventKind::error) {
      updated_usage = session.usage_totals();
    }
    screen.Post([&, apply_event, apply_extension_actions, ev, updated_usage] {
      if (updated_usage) {
        usage_totals = *updated_usage;
        ++footer_revision;
        suggestions_input.reset();
      }
      try {
        apply_event(ev);
        apply_extension_actions();
      } catch (const std::exception& e) {
        try {
          apply_event(StreamEvent{EventKind::error, e.what(), {}, {}});
        } catch (...) {
        }
      }
      if (ev.kind != EventKind::text_delta && ev.kind != EventKind::thinking_delta &&
          ev.kind != EventKind::tool_output_delta) {
        screen.RequestAnimationFrame();
      }
    });
  };

  agent.on_event = [&](const StreamEvent& ev) { post_ui(ev); };
  auto configure_extension_updates = [&](const std::shared_ptr<ExtensionRuntime>& runtime) {
    if (!runtime) {
      return;
    }
    runtime->set_tool_update([&post_ui](const std::string&, const std::string& content) {
      post_ui(StreamEvent{EventKind::status, content, {}, {}});
    });
  };
  configure_extension_updates(extensions);
  configure_extension_ui(extensions);
  bind_extensions(
      agent, extensions, cwd,
      [&](const std::string& warning) { post_ui(StreamEvent{EventKind::status, warning, {}, {}}); },
      &session, cfg);
  agent.approve_tool = [&](const niminal::ToolCall& call, const niminal::Tool& tool) {
    if (yolo_mode) {
      return true;
    }
    if (tool.read_only) {
      return true;
    }
    auto check = permissions.check(call);
    if (check == PermissionCheck::allow) {
      return true;
    }
    if (check == PermissionCheck::deny) {
      return false;
    }

    {
      std::lock_guard lock(approval.mutex);
      approval.pending = true;
      approval.resolved = false;
      approval.tool_id = call.id;
      approval.can_remember = can_remember(call);
      approval.decision = PermissionDecision::deny;
    }
    StreamEvent request{EventKind::approval_required, permission_description(call), call.name,
                        call.id};
    request.input = json::object();
    try {
      if (!call.arguments.empty()) {
        request.input = json::parse(call.arguments);
      }
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
      post_ui(StreamEvent{
          EventKind::status, "Could not save permission grant: " + std::string(e.what()), {}, {}});
    }
    return decision != PermissionDecision::deny;
  };
  bind_compaction(
      agent, session,
      [&](const std::string& msg) {
        if (msg.empty()) {
          return;
        }
        post_ui(StreamEvent{EventKind::status, msg, {}, {}});
      },
      extensions, cfg);
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
    if (worker.joinable()) {
      worker.join();
    }
  };

  auto restart_extensions = [&](bool end_current = true) {
    suggestions_input.reset();
    if (extensions && end_current) {
      auto shutdown = extensions->dispatch(
          HookEvent::session_shutdown,
          json{{"session_id", session.id}, {"workspace", cwd.string()}, {"reason", "reload"}});
      for (const auto& warning : shutdown.warnings) {
        blocks.push_back(Block{BlockKind::status, warning});
      }
      auto ended =
          extensions->dispatch(HookEvent::session_end, session_hook_payload(session.id, cwd));
      for (const auto& warning : ended.warnings) {
        blocks.push_back(Block{BlockKind::status, warning});
      }
    }
    if (extensions) {
      extensions->stop();
    }
    extensions = ExtensionRuntime::start(cwd, session.id, cancel);
    install_extension_tools(agent, extensions, allowed_tools);
    configure_extension_updates(extensions);
    configure_extension_ui(extensions);
    bind_extensions(
        agent, extensions, cwd,
        [&](const std::string& warning) {
          post_ui(StreamEvent{EventKind::status, warning, {}, {}});
        },
        &session, cfg);
    bind_compaction(
        agent, session,
        [&](const std::string& msg) {
          if (!msg.empty()) {
            post_ui(StreamEvent{EventKind::status, msg, {}, {}});
          }
        },
        extensions, cfg);
    for (const auto& warning : extensions->warnings()) {
      blocks.push_back(Block{BlockKind::status, warning});
    }
    auto started =
        extensions->dispatch(HookEvent::session_start, session_hook_payload(session.id, cwd));
    for (const auto& warning : started.warnings) {
      blocks.push_back(Block{BlockKind::status, warning});
    }
    apply_extension_actions();
  };

  auto reload_local = [&] {
    loaded_keybindings = load_keybindings();
    if (!loaded_keybindings.error.empty()) {
      blocks.push_back(Block{BlockKind::error, loaded_keybindings.error +
                                                   "\nUsing default keybindings for this launch."});
    }
    theme = load_theme(cfg.theme).value_or(resolve_theme(ThemeMode::automatic));
    markdown_cache.clear();
    if (reload_system_prompt) {
      reload_system_prompt();
    }
  };

  auto load_into_ui = [&](const std::string& note) {
    blocks.clear();
    markdown_cache.clear();
    if (!note.empty()) {
      blocks.push_back(Block{BlockKind::status, note});
    }
    if (!session.workspace.empty() && session.workspace != cwd.string()) {
      blocks.push_back(
          Block{BlockKind::status, "This session was started in " + session.workspace});
    }
    auto event_blocks = blocks_from_events(session.events);
    blocks.insert(blocks.end(), std::make_move_iterator(event_blocks.begin()),
                  std::make_move_iterator(event_blocks.end()));
  };

  auto adopt_session = [&](Session next, const std::string& note, const std::string& reason) {
    if (extensions) {
      auto shutdown =
          extensions->dispatch(HookEvent::session_shutdown, json{{"session_id", session.id},
                                                                 {"workspace", cwd.string()},
                                                                 {"reason", reason},
                                                                 {"target_session_id", next.id}});
      for (const auto& warning : shutdown.warnings) {
        blocks.push_back(Block{BlockKind::status, warning});
      }
      auto ended =
          extensions->dispatch(HookEvent::session_end, session_hook_payload(session.id, cwd));
      for (const auto& warning : ended.warnings) {
        blocks.push_back(Block{BlockKind::status, warning});
      }
      extensions->stop();
    }
    session = std::move(next);
    usage_totals = session.usage_totals();
    ++footer_revision;
    load_history();
    bind_session(agent, session);
    restore_config_from_session(cfg, session);
    apply_provider(agent, cfg);
    agent.messages = session.openai_messages();
    load_into_ui(note);
    restart_extensions(false);
  };
  auto allow_session_switch = [&](const std::string& reason, const std::string& target) {
    if (!extensions) {
      return true;
    }
    auto outcome =
        extensions->dispatch(HookEvent::session_before_switch, json{{"session_id", session.id},
                                                                    {"workspace", cwd.string()},
                                                                    {"reason", reason},
                                                                    {"target_session_id", target}});
    for (const auto& warning : outcome.warnings) {
      blocks.push_back(Block{BlockKind::status, warning});
    }
    if (!outcome.allowed) {
      blocks.push_back(Block{BlockKind::status, outcome.reason});
    }
    return outcome.allowed;
  };

  bind_session(agent, session);
  int recovered = 0;
  try {
    recovered = session.recover_interrupted_tools();
  } catch (const std::exception& e) {
    blocks.push_back(Block{BlockKind::error, e.what()});
  }
  agent.messages = session.openai_messages();
  load_into_ui((recovered != 0)
                   ? "Recovered " + std::to_string(recovered) + " interrupted tool call(s)."
                   : "");
  if (!loaded_keybindings.error.empty()) {
    blocks.push_back(Block{BlockKind::error, loaded_keybindings.error +
                                                 "\nUsing default keybindings for this launch."});
  }
  if (extensions) {
    for (const auto& warning : extensions->warnings()) {
      blocks.push_back(Block{BlockKind::status, warning});
    }
    apply_extension_actions();
  }

  auto send_prompt = [&](niminal::UserInput prompt, bool retry = false) {
    prompt.text = niminal::trim_copy(std::move(prompt.text));
    if (prompt.text.empty() && prompt.images.empty()) {
      return;
    }
    if (busy) {
      remember_input(prompt);
      {
        std::lock_guard<std::mutex> lock(steering_mu);
        steering.push_back(prompt);
      }
      return;
    }
    if (!retry) {
      remember_input(prompt);
      retry_prompt = prompt;
      retry_available = false;
    }
    join_worker();
    cancel->store(false);
    if (!retry) {
      blocks.push_back(Block{BlockKind::user, compose_input_preview(prompt)});
    }
    busy = true;
    turn_failed = false;
    activity = "Thinking…";
    worker = std::thread([&agent, &busy, &ui_alive, post_ui, prompt = std::move(prompt), retry] {
      try {
        agent.run(prompt, !retry);
      } catch (const std::exception& e) {
        if (ui_alive) {
          post_ui(StreamEvent{EventKind::error, e.what(), {}, {}});
          post_ui(StreamEvent{EventKind::done, {}, {}, {}});
        } else {
          busy = false;
        }
      }
    });
  };
  deliver_extension_now = [&](std::string prompt) { send_prompt(std::move(prompt)); };
  auto run_slash = [&](const std::string& cmd, const std::string& arg, bool extension_request) {
    SlashHost slash{
        agent,
        session,
        cfg,
        theme,
        permissions,
        cwd,
        keybindings,
        extensions,
        busy,
        yolo_mode,
        retry_available,
        retry_prompt,
        settings_open,
        settings_i,
        settings_edit,
        settings_error,
        blocks,
        pending_changes,
        [&](const std::string& message) { flash_footer(message); },
        [&](const std::string& reason, const std::string& target) {
          return allow_session_switch(reason, target);
        },
        [&](Session next, const std::string& note, const std::string& reason) {
          adopt_session(std::move(next), note, reason);
        },
        [&] { restart_extensions(); },
        [&] { reload_local(); },
        [&] { apply_extension_actions(); },
        [&](niminal::UserInput input, bool retry) { send_prompt(std::move(input), retry); },
        [&] {
          cancel->store(true);
          ui_alive = false;
          screen.Exit();
        },
        [&](std::string text) {
          draft = std::move(text);
          cursor = static_cast<int>(draft.size());
        },
    };
    return execute_slash(slash, cmd, arg, extension_request);
  };
  apply_pending_changes = [&] {
    join_worker();
    auto changes = std::exchange(pending_changes, {});
    for (const auto& [cmd, arg] : changes) {
      run_slash(cmd, arg, false);
    }
  };
  handle_turn_idle = [&] {
    if (send_queue_after_stop) {
      send_queue_after_stop = false;
      plain_interrupt_pending = false;
      niminal::UserInput prompt;
      {
        std::lock_guard<std::mutex> lock(steering_mu);
        if (!steering.empty()) {
          prompt = std::move(steering.front());
          steering.erase(steering.begin());
        }
      }
      if (!prompt.text.empty() || !prompt.images.empty()) {
        send_prompt(std::move(prompt));
      }
      return;
    }
    if (plain_interrupt_pending) {
      plain_interrupt_pending = false;
      std::lock_guard<std::mutex> lock(steering_mu);
      steering.clear();
      follow_up.clear();
    }
  };
  for (auto& message : idle_extension_messages) {
    deliver_extension_now(std::move(message));
  }
  idle_extension_messages.clear();

  auto shell_env = [&] { return make_shell_env(session, agent, cfg); };

  int user_bash_seq = 0;
  auto run_user_bash = [&](std::string command, bool exclude_from_context,
                           std::string history_line) {
    remember_input(niminal::UserInput{std::move(history_line)});
    if (busy || user_bash_running) {
      blocks.push_back(Block{BlockKind::status, busy_wait_message(keybindings)});
      return;
    }
    join_worker();
    cancel->store(false);
    stick_bottom = true;
    transcript_y = 1.F;
    const std::string tool_id = "ubash-" + std::to_string(++user_bash_seq);
    post_ui(StreamEvent{EventKind::tool_call, json{{"command", command}}.dump(), "bash", tool_id});
    user_bash_running = true;
    activity = "Running bash…";
    worker = std::thread([&, command = std::move(command), exclude_from_context, tool_id] {
      try {
        const auto env = shell_env();
        auto output = run_bash(
            command, cwd, 120, cancel,
            [&](const std::string& snapshot) {
              post_ui(StreamEvent{EventKind::tool_output_delta, snapshot, "bash", tool_id});
            },
            env);
        run_on_ui([&] {
          session.add_bash(command, output, exclude_from_context);
          agent.messages = session.openai_messages();
          workspace.invalidate_listing();
        });
        post_ui(StreamEvent{EventKind::tool_result, output, "bash", tool_id});
      } catch (const std::exception& e) {
        StreamEvent result{EventKind::tool_result, e.what(), "bash", tool_id};
        result.is_error = true;
        post_ui(result);
      }
      user_bash_running = false;
      activity.clear();
    });
  };

  auto start_turn = [&](std::string prompt) {
    prompt = niminal::trim_copy(std::move(prompt));
    if (prompt.empty()) {
      return;
    }
    stick_bottom = true;
    transcript_y = 1.F;

    if (auto bash = parse_user_bash(prompt)) {
      run_user_bash(std::move(bash->command), bash->exclude_from_context, std::move(prompt));
      return;
    }

    const auto [cmd, arg] = split_slash(prompt);
    const bool skill_request = cmd.starts_with("/skill:");
    const bool extension_request = is_extension_slash(extensions, cmd);
    if (auto skill_error = skill_slash_error(cwd, cmd)) {
      blocks.push_back(Block{BlockKind::error, *skill_error});
      return;
    }

    if (prompt[0] == '/' && !skill_request && !extension_request) {
      if (auto resolved = resolve_prompt_template(cwd, prompt, cmd, arg)) {
        send_prompt(niminal::UserInput{std::move(*resolved)});
        return;
      }
    }

    if (prompt[0] == '/' && !skill_request) {
      if (run_slash(cmd, arg, extension_request)) {
        return;
      }
    }

    send_prompt(niminal::UserInput{std::move(prompt), std::exchange(draft_images, json::array())});
  };

  InputOption input_opt;
  input_opt.multiline = true;
  input_opt.cursor_position = &cursor;
  input_opt.transform = [&theme](InputState state) {
    state.element |= color(theme.input_fg);
    if (state.is_placeholder) {
      state.element |= dim;
    }
    if (state.focused) {
      state.element |= bgcolor(theme.input_bg);
    } else if (state.hovered) {
      state.element |= bgcolor(theme.hover_bg);
    }
    return state.element;
  };
  auto input = Input(&draft, "describe a change", input_opt);
  auto input_transform = input_opt.transform;
  auto wrapped_input = Renderer(input, [&] {
    if (draft.empty()) {
      return input->Render();
    }

    const int cursor_pos = std::clamp(cursor, 0, static_cast<int>(draft.size()));
    const int terminal_width = screen.dimx() > 0 ? screen.dimx() : Terminal::Size().dimx;
    Elements rows;
    size_t line_start = 0;
    while (true) {
      const auto newline = draft.find('\n', line_start);
      const size_t line_end = newline == std::string::npos ? draft.size() : newline;
      const auto line = draft.substr(line_start, line_end - line_start);
      Elements glyphs;
      size_t byte = line_start;
      for (const auto& glyph : Utf8ToGlyphs(line)) {
        auto cell = text(glyph);
        if (!glyph.empty() && byte == static_cast<size_t>(cursor_pos)) {
          cell =
              input->Focused() ? focusCursorBarBlinking(std::move(cell)) : focus(std::move(cell));
        }
        glyphs.push_back(std::move(cell));
        byte += glyph.size();
      }
      if (line.empty() || byte == static_cast<size_t>(cursor_pos)) {
        auto cell = text(" ");
        cell = input->Focused() ? focusCursorBarBlinking(std::move(cell)) : focus(std::move(cell));
        glyphs.push_back(std::move(cell));
      }
      rows.push_back(hflow(std::move(glyphs)));
      if (line_end == draft.size()) {
        break;
      }
      line_start = line_end + 1;
    }

    auto element =
        vbox(std::move(rows)) | size(WIDTH, LESS_THAN, std::max(1, terminal_width - 2)) | frame;
    return input_transform({std::move(element), false, input->Focused(), false}) | xflex;
  });

  auto layout = Container::Vertical({wrapped_input});
  std::optional<std::tuple<std::string, std::string, std::string, size_t>> footer_key;
  std::string think;
  std::string usage;
  auto view = Renderer(layout, [&] {
    card_boxes.assign(blocks.size(), Box{});
    markdown_cache.resize(blocks.size());
    Elements entries;
    for (size_t i = 0; i < blocks.size(); ++i) {
      const auto& block = blocks[i];
      if (is_card_block(block.kind)) {
        entries.push_back(render_transcript_card(block, theme, card_boxes[i]));
      } else if (block.kind == BlockKind::user) {
        entries.push_back(render_user_message(block, theme));
      } else if (block.kind == BlockKind::assistant) {
        auto& cached = markdown_cache[i];
        if (!cached.second || cached.first != block.text) {
          cached = {block.text, render_markdown(block.text, theme)};
        }
        entries.push_back(cached.second);
      } else {
        auto label = block_label(block.kind);
        auto body = paragraph_preserving_whitespace(block.text) | block_style(block.kind, theme);
        if (label && *label) {
          entries.push_back(vbox({text(label) | bold | block_style(block.kind, theme), body}));
        } else {
          entries.push_back(body);
        }
      }
      const bool next_is_card = i + 1 < blocks.size() && is_card_block(blocks[i + 1].kind);
      if (!(is_card_block(block.kind) && next_is_card)) {
        entries.push_back(text(""));
      }
    }

    const auto extension_statuses =
        extensions ? extensions->statuses() : std::vector<ExtensionStatus>{};
    const auto extension_widgets =
        extensions ? extensions->widgets() : std::vector<ExtensionWidget>{};
    std::vector<std::pair<size_t, size_t>> extension_action_slots;
    for (size_t widget_index = 0; widget_index < extension_widgets.size(); ++widget_index) {
      for (size_t action_index = 0; action_index < extension_widgets[widget_index].actions.size();
           ++action_index) {
        extension_action_slots.emplace_back(widget_index, action_index);
      }
    }
    if (extension_action_focus) {
      if (extension_action_slots.empty()) {
        extension_action_focus.reset();
      } else {
        *extension_action_focus %= extension_action_slots.size();
      }
    }

    std::string activity_line;
    if (busy || !activity.empty()) {
      if (busy) {
        static const char* kSpin[] = {"⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"};
        auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(
                      std::chrono::steady_clock::now().time_since_epoch())
                      .count();
        activity_line = kSpin[(ms / 200) % 10];
        activity_line += ' ';
      }
      activity_line += activity.empty() ? (cancel->load() ? "Stopping…" : "Thinking…") : activity;
      if (activity_started) {
        const auto elapsed = std::chrono::duration_cast<std::chrono::seconds>(
                                 std::chrono::steady_clock::now() - *activity_started)
                                 .count();
        activity_line += " · " + std::to_string(elapsed) + "s";
      }
      int queued = 0;
      {
        std::lock_guard<std::mutex> lock(steering_mu);
        queued = static_cast<int>(steering.size() + follow_up.size());
      }
      if (queued > 0) {
        activity_line += "  ·  queued " + std::to_string(queued);
      }
    }
    if (!footer_notice.empty()) {
      if (std::chrono::steady_clock::now() < footer_notice_until) {
        if (!activity_line.empty()) {
          activity_line += "  ·  ";
        }
        activity_line += footer_notice;
      } else {
        footer_notice.clear();
        footer_notice_active = false;
      }
    }

    const auto next_footer_key =
        std::tuple{agent.provider, agent.model, cfg.thinking, footer_revision};
    if (footer_key != next_footer_key) {
      think = thinking_status(agent.provider, agent.model, cfg.thinking);
      if (think.empty()) {
        think = thinking_choices(agent.provider, agent.model).empty()
                    ? std::string()
                    : (cfg.thinking.empty() ? "default" : "off");
      }
      usage = format_usage_line(usage_totals);
      if (!usage.empty()) {
        auto cost = lookup_model_cost(agent.provider, agent.model);
        if (cost.known) {
          usage += "  " + format_cost_usd(usage_cost_usd(usage_totals, cost));
        }
      }
      footer_key = next_footer_key;
    }

    const auto& suggestions = current_suggestions();
    Elements suggest_rows;
    for (int i = 0; i < static_cast<int>(suggestions.size()); ++i) {
      auto line = text(suggestions[static_cast<size_t>(i)].label) | dim;
      if (i == suggest_i) {
        line = line | inverted;
      }
      suggest_rows.push_back(std::move(line));
    }

    auto transcript = vbox(std::move(entries));
    transcript->ComputeRequirement();
    transcript_rows = std::max(1, transcript->requirement().min_y);
    Elements stack;
    stack.push_back(transcript | focusPositionRelative(0.F, stick_bottom ? 1.F : transcript_y) |
                    vscroll_indicator | yframe | yflex);
    stack.push_back(separator());
    if (settings_open) {
      Elements setting_rows;
      setting_rows.push_back(text("Settings  " + config_path().string()) | bold |
                             color(theme.accent));
      for (size_t i = 0; i < setting_count(); ++i) {
        const auto* spec = setting_at(i);
        if (spec == nullptr) {
          continue;
        }
        std::string line = std::string(spec->label) + ": ";
        if (settings_edit && static_cast<int>(i) == settings_i) {
          line += *settings_edit + "▌";
        } else {
          line += format_setting_value(cfg, spec->field);
        }
        auto row = text(line);
        if (static_cast<int>(i) == settings_i) {
          row = row | inverted;
        }
        setting_rows.push_back(std::move(row));
      }
      if (!settings_error.empty()) {
        setting_rows.push_back(text(settings_error) | color(theme.error));
      }
      const auto* selected = setting_at(static_cast<size_t>(settings_i));
      std::string help = settings_edit
                             ? "Enter save  Esc cancel edit"
                             : "↑/↓ select  Enter edit/toggle/cycle  ←/→ cycle  Esc close";
      if (!settings_edit && selected != nullptr) {
        if (selected->kind == SettingKind::toggle) {
          help += "  Space toggles";
        } else if (selected->kind == SettingKind::text || selected->kind == SettingKind::integer) {
          help += "  type to edit";
        }
      }
      setting_rows.push_back(text(help) | dim);
      stack.push_back(vbox(std::move(setting_rows)));
    } else if (ask_user_open) {
      Elements question_rows;
      question_rows.push_back(paragraph(ask_user_question) | bold | color(theme.emphasis));
      question_rows.push_back(text(""));
      for (size_t i = 0; i <= ask_user_options.size(); ++i) {
        const bool is_other = i == ask_user_options.size();
        const bool selected = static_cast<int>(i) == ask_user_i;
        auto marker = text(selected ? " › " : "   ") | bold | color(theme.accent);
        auto number = text(std::to_string(i + 1) + ". ") | color(theme.muted);
        auto label = is_other ? text("Other…") | italic : paragraph(ask_user_options[i]);
        label = selected ? label | bold | color(theme.accent) : label;
        auto row = hbox({marker, number, label | flex});
        if (selected) {
          row = row | bgcolor(theme.hover_bg);
        }
        question_rows.push_back(std::move(row));
      }
      if (ask_user_editing_other) {
        question_rows.push_back(text(""));
        Elements answer_glyphs{text(" › ") | bold | color(theme.accent)};
        size_t byte = 0;
        for (const auto& glyph : Utf8ToGlyphs(ask_user_answer)) {
          if (byte == static_cast<size_t>(ask_user_cursor)) {
            answer_glyphs.push_back(focusCursorBarBlinking(text(glyph)));
          } else {
            answer_glyphs.push_back(text(glyph));
          }
          byte += glyph.size();
        }
        if (byte == static_cast<size_t>(ask_user_cursor)) {
          answer_glyphs.push_back(focusCursorBarBlinking(text(" ")));
        }
        question_rows.push_back(hflow(std::move(answer_glyphs)) | bgcolor(theme.input_bg) |
                                color(theme.input_fg));
      }
      question_rows.push_back(text(""));
      question_rows.push_back(
          text(ask_user_editing_other
                   ? "enter submit · esc cancel · pgup/pgdn scroll"
                   : "↑/↓ choose · enter select · esc cancel · pgup/pgdn scroll") |
          color(theme.muted));
      stack.push_back(window(text(" Question ") | bold | color(theme.accent),
                             vbox(std::move(question_rows)), ROUNDED));
    } else {
      if (!suggest_rows.empty()) {
        stack.push_back(vbox(std::move(suggest_rows)));
      }
      if (!extension_widgets.empty()) {
        Elements widget_rows;
        size_t action_offset = 0;
        for (size_t widget_index = 0; widget_index < extension_widgets.size(); ++widget_index) {
          const auto& widget = extension_widgets[widget_index];
          Elements rows;
          for (const auto& item : widget.content) {
            const auto type = item.value("type", std::string());
            if (type == "text") {
              auto line = text(item.value("text", std::string()));
              const auto style = item.value("style", std::string("muted"));
              if (style == "accent") {
                line = line | color(theme.accent);
              } else if (style == "success") {
                line = line | color(theme.add);
              } else if (style == "warning") {
                line = line | color(theme.emphasis);
              } else if (style == "error") {
                line = line | color(theme.error);
              } else {
                line = line | color(theme.muted);
              }
              rows.push_back(std::move(line));
            } else if (type == "list") {
              for (const auto& entry : item["items"]) {
                const auto state = entry.value("state", std::string("pending"));
                const bool done = state == "done";
                auto marker = text(done ? "✓ " : (state == "active" ? "› " : "· "));
                marker = marker |
                         color(done ? theme.add : (state == "active" ? theme.accent : theme.muted));
                auto label = paragraph(entry.value("text", std::string()));
                if (done) {
                  label = label | dim;
                }
                rows.push_back(hbox({text("  "), marker, label}));
              }
            } else if (type == "progress") {
              const double maximum = item["max"].get<double>();
              const double value = std::clamp(item["value"].get<double>(), 0.0, maximum);
              const auto label = item.value("label", std::string());
              auto meter = gauge(static_cast<float>(value / maximum)) | size(WIDTH, EQUAL, 16) |
                           color(theme.add);
              rows.push_back(label.empty() ? std::move(meter)
                                           : hbox({text(label + " "), std::move(meter)}));
            }
          }
          for (size_t action_index = 0; action_index < widget.actions.size(); ++action_index) {
            const size_t slot = action_offset + action_index;
            const bool selected = extension_action_focus && *extension_action_focus == slot;
            auto row =
                text(std::string(selected ? " › " : "   ") + "[" +
                     std::to_string(action_index + 1) + "] " + widget.actions[action_index].label);
            row = selected ? row | bold | color(theme.accent) | bgcolor(theme.hover_bg)
                           : row | color(theme.muted);
            rows.push_back(std::move(row));
          }
          action_offset += widget.actions.size();
          if (!widget.actions.empty()) {
            rows.push_back(text(keybindings.label(KeyAction::complete) +
                                " focus · ↑/↓ choose · enter run · esc close") |
                           dim);
          }
          const auto title =
              widget.title.empty() ? widget.extension + " · " + widget.key : widget.title;
          widget_rows.push_back(window(text(" " + title + " ") | bold | color(theme.accent),
                                       vbox(std::move(rows)), ROUNDED));
        }
        stack.push_back(vbox(std::move(widget_rows)));
      }
      Elements footer_elements;
      if (!activity_line.empty()) {
        footer_elements.push_back(text(activity_line) | color(theme.accent));
      }
      for (const auto& status : extension_statuses) {
        if (!footer_elements.empty()) {
          footer_elements.push_back(text("  ·  ") | color(theme.muted));
        }
        for (const auto& segment : status.segments) {
          auto part = text(segment.text);
          if (segment.style == "muted") {
            part = part | color(theme.muted);
          } else if (segment.style == "success") {
            part = part | color(theme.add);
          } else if (segment.style == "warning") {
            part = part | color(theme.emphasis);
          } else if (segment.style == "error") {
            part = part | color(theme.error);
          } else if (segment.style == "emphasis") {
            part = part | bold | color(theme.emphasis);
          } else {
            part = part | color(theme.accent);
          }
          footer_elements.push_back(std::move(part));
        }
      }
      stack.push_back(footer_elements.empty() ? text(" ") : hbox(std::move(footer_elements)));
      {
        std::vector<std::string> steering_preview;
        std::vector<std::string> follow_up_preview;
        {
          std::lock_guard<std::mutex> lock(steering_mu);
          for (const auto& queued : steering) {
            steering_preview.push_back(compose_input_preview(queued));
          }
          for (const auto& queued : follow_up) {
            follow_up_preview.push_back(compose_input_preview(queued));
          }
        }
        auto queue_preview = render_queue_preview(steering_preview, follow_up_preview, keybindings);
        if (!queue_preview.empty()) {
          stack.push_back(vbox(std::move(queue_preview)));
        }
      }
      stack.push_back(separatorLight() | dim);
      if (!draft_images.empty()) {
        std::string names = "Images: ";
        for (const auto& image : draft_images) {
          if (names != "Images: ") {
            names += ", ";
          }
          names += image.value("name", "image");
        }
        stack.push_back(text(names + "  (Backspace with empty text removes last)") | dim);
      }
      stack.push_back(hbox({text(busy ? "…" : "› ") | bold,
                            wrapped_input->Render() | xflex | size(HEIGHT, LESS_THAN, 8)}));
    }
    stack.push_back(hbox({
        text(usage.empty() ? "↑0  ↓0" : usage) | dim,
        filler(),
        text(agent.provider + (agent.model_runtime.empty() ? "" : "/" + agent.model_runtime) + "/" +
             agent.model + (yolo_mode ? " [yolo]" : "")) |
            color(theme.accent),
        text(think.empty() ? std::string() : (":" + think)) | dim,
    }));
    return vbox(std::move(stack));
  });

  auto insert_draft = [&](const std::string& text) {
    if (text.empty()) {
      return;
    }
    int pos = std::clamp(cursor, 0, static_cast<int>(draft.size()));
    draft.insert(static_cast<size_t>(pos), text);
    cursor = pos + static_cast<int>(text.size());
  };

  auto paste_clipboard_into_draft = [&] {
    try {
      if (auto image = paste_image_from_clipboard()) {
        draft_images.push_back(std::move(*image));
      } else {
        insert_draft(paste_from_clipboard());
      }
    } catch (const std::exception& ex) {
      flash_footer(ex.what());
    }
  };

  auto pop_last_steering_to_composer = [&]() -> bool {
    niminal::UserInput message;
    {
      std::lock_guard<std::mutex> lock(steering_mu);
      if (steering.empty()) {
        return false;
      }
      message = std::move(steering.back());
      steering.pop_back();
    }
    if (!draft.empty()) {
      draft = std::move(message.text) + "\n\n" + draft;
    } else {
      draft = std::move(message.text);
    }
    for (auto& image : message.images) {
      draft_images.push_back(std::move(image));
    }
    cursor = static_cast<int>(draft.size());
    history_i = -1;
    return true;
  };

  auto resolve_approval = [&](PermissionDecision decision) {
    std::lock_guard lock(approval.mutex);
    if (!approval.pending || approval.resolved) {
      return false;
    }
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
    auto pressed = [&](KeyAction action) { return keybindings.matches(action, e); };
    auto scroll_transcript = [&](bool up, bool page) {
      const float step = page ? 0.35F : 1.F / static_cast<float>(transcript_rows);
      transcript_y = std::clamp(transcript_y + (up ? -step : step), 0.F, 1.F);
      stick_bottom = !up && transcript_y == 1.F;
    };
    if (ask_user_open) {
      if (is_wheel_up(e) || pressed(KeyAction::scroll_up)) {
        scroll_transcript(true, pressed(KeyAction::scroll_up));
        return true;
      }
      if (is_wheel_down(e) || pressed(KeyAction::scroll_down)) {
        scroll_transcript(false, pressed(KeyAction::scroll_down));
        return true;
      }
      if (pressed(KeyAction::cancel)) {
        finish_ask_user(niminal::ToolResult{"User cancelled the question."});
        return true;
      }
      if (pressed(KeyAction::quit)) {
        finish_ask_user(niminal::ToolResult{"interrupted"});
        cancel->store(true);
        ui_alive = false;
        screen.Exit();
        return true;
      }
      if (ask_user_editing_other) {
        if (pressed(KeyAction::submit)) {
          if (!ask_user_answer.empty()) {
            finish_ask_user(niminal::ToolResult{ask_user_answer});
          }
          return true;
        }
        if (e == Event::Backspace && ask_user_cursor > 0) {
          const int previous = cursor_glyph_left(ask_user_answer, ask_user_cursor);
          ask_user_answer.erase(static_cast<size_t>(previous),
                                static_cast<size_t>(ask_user_cursor - previous));
          ask_user_cursor = previous;
          return true;
        }
        if (e == Event::ArrowLeft) {
          ask_user_cursor = cursor_glyph_left(ask_user_answer, ask_user_cursor);
          return true;
        }
        if (e == Event::ArrowRight) {
          ask_user_cursor = cursor_glyph_right(ask_user_answer, ask_user_cursor);
          return true;
        }
        if (pressed(KeyAction::word_left)) {
          ask_user_cursor = cursor_word_left(ask_user_answer, ask_user_cursor);
          return true;
        }
        if (pressed(KeyAction::word_right)) {
          ask_user_cursor = cursor_word_right(ask_user_answer, ask_user_cursor);
          return true;
        }
        if (pressed(KeyAction::draft_start)) {
          ask_user_cursor = 0;
          return true;
        }
        if (pressed(KeyAction::draft_end)) {
          ask_user_cursor = static_cast<int>(ask_user_answer.size());
          return true;
        }
        if (e.is_character()) {
          const auto character = e.character();
          ask_user_answer.insert(static_cast<size_t>(ask_user_cursor), character);
          ask_user_cursor += static_cast<int>(character.size());
          return true;
        }
        return true;
      }
      if (pressed(KeyAction::previous)) {
        ask_user_i = (ask_user_i + static_cast<int>(ask_user_options.size())) %
                     static_cast<int>(ask_user_options.size() + 1);
        return true;
      }
      if (pressed(KeyAction::next)) {
        ask_user_i = (ask_user_i + 1) % static_cast<int>(ask_user_options.size() + 1);
        return true;
      }
      if (pressed(KeyAction::submit)) {
        if (ask_user_i == static_cast<int>(ask_user_options.size())) {
          ask_user_editing_other = true;
          ask_user_answer.clear();
          ask_user_cursor = 0;
        } else {
          finish_ask_user(niminal::ToolResult{ask_user_options[static_cast<size_t>(ask_user_i)]});
        }
        return true;
      }
      return true;
    }
    if (approval_pending()) {
      if (pressed(KeyAction::allow_once)) {
        resolve_approval(PermissionDecision::allow_once);
      } else if (pressed(KeyAction::allow_session)) {
        resolve_approval(PermissionDecision::allow_session);
      } else if (pressed(KeyAction::allow_project)) {
        bool allowed = false;
        {
          std::lock_guard lock(approval.mutex);
          allowed = approval.can_remember;
        }
        if (!allowed) {
          return true;
        }
        resolve_approval(PermissionDecision::allow_project);
      } else if (pressed(KeyAction::deny)) {
        resolve_approval(PermissionDecision::deny);
      } else if (pressed(KeyAction::quit)) {
        resolve_approval(PermissionDecision::deny);
        cancel->store(true);
        ui_alive = false;
        screen.Exit();
      } else {
        return true;
      }
      return true;
    }
    if (settings_open) {
      const int count = static_cast<int>(setting_count());
      settings_i = std::clamp(settings_i, 0, std::max(0, count - 1));
      const auto* spec = setting_at(static_cast<size_t>(settings_i));
      if (settings_edit) {
        if (pressed(KeyAction::submit)) {
          if (spec != nullptr) {
            persist_settings(apply_setting_value(cfg, spec->field, *settings_edit));
            if (settings_error.empty()) {
              settings_edit = std::nullopt;
            }
          }
          return true;
        }
        if (pressed(KeyAction::cancel)) {
          settings_edit = std::nullopt;
          settings_error.clear();
          return true;
        }
        if (e == Event::Backspace) {
          if (!settings_edit->empty()) {
            settings_edit->pop_back();
          }
          return true;
        }
        if (e.is_character()) {
          settings_edit->append(e.character());
          return true;
        }
        return true;
      }
      if (pressed(KeyAction::cancel)) {
        settings_open = false;
        settings_error.clear();
        return true;
      }
      if (pressed(KeyAction::quit)) {
        cancel->store(true);
        ui_alive = false;
        screen.Exit();
        return true;
      }
      if (pressed(KeyAction::previous)) {
        settings_i = (settings_i + count - 1) % std::max(1, count);
        return true;
      }
      if (pressed(KeyAction::next)) {
        settings_i = (settings_i + 1) % std::max(1, count);
        return true;
      }
      if (spec != nullptr) {
        if (e == Event::ArrowLeft && spec->kind == SettingKind::cycle) {
          persist_settings(cycle_setting(cfg, spec->field, -1, agent.provider, agent.model));
          return true;
        }
        if (e == Event::ArrowRight && spec->kind == SettingKind::cycle) {
          persist_settings(cycle_setting(cfg, spec->field, 1, agent.provider, agent.model));
          return true;
        }
        if (e == Event::Character(' ') && spec->kind == SettingKind::toggle) {
          persist_settings(toggle_setting(cfg, spec->field));
          return true;
        }
        if (pressed(KeyAction::submit)) {
          if (spec->kind == SettingKind::toggle) {
            persist_settings(toggle_setting(cfg, spec->field));
          } else if (spec->kind == SettingKind::cycle) {
            persist_settings(cycle_setting(cfg, spec->field, 1, agent.provider, agent.model));
          } else if (spec->kind == SettingKind::text || spec->kind == SettingKind::integer) {
            settings_edit = edit_initial_value(cfg, spec->field);
            settings_error.clear();
          }
          return true;
        }
      }
      return true;
    }
    std::vector<std::tuple<std::string, std::string, std::string>> extension_actions;
    if (extensions) {
      for (const auto& widget : extensions->widgets()) {
        for (const auto& action : widget.actions) {
          extension_actions.emplace_back(widget.extension, widget.key, action.id);
        }
      }
    }
    if (extension_action_focus && extension_actions.empty()) {
      extension_action_focus.reset();
    }
    if (extension_action_focus) {
      *extension_action_focus %= extension_actions.size();
      if (pressed(KeyAction::cancel)) {
        extension_action_focus.reset();
        return true;
      }
      if (pressed(KeyAction::quit)) {
        cancel->store(true);
        ui_alive = false;
        screen.Exit();
        return true;
      }
      if (pressed(KeyAction::submit)) {
        const auto& [extension, widget, action] = extension_actions[*extension_action_focus];
        if (!extensions->activate_widget_action(extension, widget, action)) {
          flash_footer("Extension action is no longer available.");
        }
        extension_action_focus.reset();
        return true;
      }
      if (pressed(KeyAction::complete) || pressed(KeyAction::next)) {
        *extension_action_focus = (*extension_action_focus + 1) % extension_actions.size();
        return true;
      }
      if (pressed(KeyAction::previous)) {
        *extension_action_focus =
            (*extension_action_focus + extension_actions.size() - 1) % extension_actions.size();
        return true;
      }
      extension_action_focus.reset();
    } else if (pressed(KeyAction::complete) && draft.empty() && current_suggestions().empty() &&
               !extension_actions.empty()) {
      extension_action_focus = 0;
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
    if (pasting && (e.is_character() || e == Event::Return || e == Event::Tab)) {
      insert_draft(e.input());
      return true;
    }
    if (pressed(KeyAction::paste)) {
      paste_clipboard_into_draft();
      return true;
    }
    if (pressed(KeyAction::external_editor)) {
      try {
        std::string edited;
        with_restored_io([&] { edited = edit_text_externally(draft, cfg.editor); });
        draft = std::move(edited);
        cursor = static_cast<int>(draft.size());
        history_i = -1;
      } catch (const std::exception& ex) {
        flash_footer(ex.what());
      }
      return true;
    }
    if (e.is_mouse() && e.mouse().motion == Mouse::Pressed && e.mouse().button == Mouse::Left) {
      card_press_index = card_at(blocks, card_boxes, e.mouse().x, e.mouse().y);
      mouse_press = {e.mouse().x, e.mouse().y};
      return false;
    }
    if (e.is_mouse() && e.mouse().motion == Mouse::Released && e.mouse().button == Mouse::Left) {
      const bool dragged = mouse_press && is_drag_gesture(mouse_press->first, mouse_press->second,
                                                          e.mouse().x, e.mouse().y);
      mouse_press = std::nullopt;
      auto sel = screen.GetSelection();
      if (dragged && !sel.empty()) {
        copy_to_clipboard(sel);
        flash_footer("Copied to clipboard.");
        card_press_index = std::nullopt;
        return true;
      }
      if (card_press_index) {
        auto release = card_at(blocks, card_boxes, e.mouse().x, e.mouse().y);
        if (release && *release == *card_press_index && *release < blocks.size() &&
            is_card_block(blocks[*release].kind)) {
          blocks[*release].expanded = !blocks[*release].expanded;
          card_press_index = std::nullopt;
          return true;
        }
      }
      card_press_index = std::nullopt;
    }
    if (e.is_mouse() && e.mouse().motion == Mouse::Pressed &&
        (e.mouse().button == Mouse::Middle || e.mouse().button == Mouse::Right)) {
      paste_clipboard_into_draft();
      return true;
    }
    if (e == Event::Backspace && draft.empty() && !draft_images.empty()) {
      draft_images.erase(draft_images.end() - 1);
      return true;
    }
    if (pressed(KeyAction::toggle_last)) {
      const auto index = last_card_index(blocks);
      if (index >= 0) {
        blocks[static_cast<size_t>(index)].expanded = !blocks[static_cast<size_t>(index)].expanded;
      }
      return true;
    }
    if (pressed(KeyAction::toggle_all)) {
      bool any_collapsed = false;
      for (const auto& block : blocks) {
        if (is_card_block(block.kind) && !block.expanded) {
          any_collapsed = true;
          break;
        }
      }
      for (auto& block : blocks) {
        if (is_card_block(block.kind)) {
          block.expanded = any_collapsed;
        }
      }
      return true;
    }
    if (is_wheel_up(e) || pressed(KeyAction::scroll_up)) {
      scroll_transcript(true, pressed(KeyAction::scroll_up));
      return true;
    }
    if (is_wheel_down(e) || pressed(KeyAction::scroll_down)) {
      scroll_transcript(false, pressed(KeyAction::scroll_down));
      return true;
    }
    const auto& suggestions = current_suggestions();
    if (!suggestions.empty()) {
      if (pressed(KeyAction::next)) {
        suggest_i = (suggest_i + 1) % static_cast<int>(suggestions.size());
        return true;
      }
      if (pressed(KeyAction::previous)) {
        suggest_i = (suggest_i + static_cast<int>(suggestions.size()) - 1) %
                    static_cast<int>(suggestions.size());
        return true;
      }
      if (pressed(KeyAction::complete)) {
        apply_suggestion(suggestions[static_cast<size_t>(suggest_i)]);
        return true;
      }
      if (pressed(KeyAction::complete_previous)) {
        suggest_i = (suggest_i + static_cast<int>(suggestions.size()) - 1) %
                    static_cast<int>(suggestions.size());
        apply_suggestion(suggestions[static_cast<size_t>(suggest_i)]);
        return true;
      }
    } else {
      if (pressed(KeyAction::previous)) {
        history_prev();
        return true;
      }
      if (pressed(KeyAction::next)) {
        history_next();
        return true;
      }
    }
    if (pressed(KeyAction::word_left)) {
      cursor = cursor_word_left(draft, cursor);
      return true;
    }
    if (pressed(KeyAction::word_right)) {
      cursor = cursor_word_right(draft, cursor);
      return true;
    }
    if (pressed(KeyAction::draft_start)) {
      cursor = 0;
      return true;
    }
    if (pressed(KeyAction::draft_end)) {
      cursor = static_cast<int>(draft.size());
      return true;
    }
    if (pressed(KeyAction::edit_queued)) {
      if (pop_last_steering_to_composer()) {
        return true;
      }
    }
    if (pressed(KeyAction::newline)) {
      int pos = std::clamp(cursor, 0, static_cast<int>(draft.size()));
      draft.insert(static_cast<size_t>(pos), 1, '\n');
      cursor = pos + 1;
      return true;
    }
    if (pressed(KeyAction::submit)) {
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
      if (prompt.empty() && !draft_images.empty()) {
        send_prompt(niminal::UserInput{"", std::exchange(draft_images, json::array())});
      } else {
        start_turn(std::move(prompt));
      }
      return true;
    }
    if (pressed(KeyAction::cancel)) {
      if (user_bash_running) {
        cancel->store(true);
        activity = "Stopping…";
        return true;
      }
      if (busy) {
        bool has_steering = false;
        {
          std::lock_guard<std::mutex> lock(steering_mu);
          has_steering = !steering.empty();
        }
        cancel->store(true);
        activity = "Stopping…";
        if (has_steering) {
          send_queue_after_stop = true;
          plain_interrupt_pending = false;
        } else {
          plain_interrupt_pending = true;
          send_queue_after_stop = false;
        }
        return true;
      }
      draft.clear();
      draft_images = json::array();
      cursor = 0;
      history_i = -1;
      return true;
    }
    if (pressed(KeyAction::quit)) {
      cancel->store(true);
      ui_alive = false;
      screen.Exit();
      return true;
    }
    // FTXUI otherwise gives retired Ctrl-Left/Right draft-jump keys word movement.
    if (e == Event::ArrowLeftCtrl || e == Event::ArrowRightCtrl) {
      return true;
    }
    return false;
  });

  load_catalog();
  std::thread catalog_thread;
  if (catalog_stale()) {
    catalog_thread = std::thread([&] {
      refresh_catalog();
      if (!ui_alive) {
        return;
      }
      screen.Post([&] {
        ++footer_revision;
        suggestions_input.reset();
        screen.RequestAnimationFrame();
      });
    });
  }
  std::thread extension_thread([&] {
    while (ui_alive) {
      std::this_thread::sleep_for(std::chrono::milliseconds(200));
      if (!ui_alive) {
        break;
      }
      const bool extension_changed = extensions && extensions->pump();
      if (!busy && !extension_changed && !footer_notice_active) {
        continue;
      }
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
  finish_ask_user(niminal::ToolResult{"interrupted"});
  resolve_approval(PermissionDecision::deny);
  join_worker();
  if (catalog_thread.joinable()) {
    catalog_thread.join();
  }
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
  if (ask_user_allowed) {
    agent.tools.erase(
        std::remove_if(agent.tools.begin(), agent.tools.end(),
                       [](const niminal::Tool& tool) { return tool.name == "ask_user"; }),
        agent.tools.end());
  }
  return 0;
}

} // namespace niminal::app
