#include "compaction.hpp"
#include "extensions.hpp"

#include <niminal/openai.hpp>

#include <algorithm>
#include <sstream>

namespace niminal::app {
using json = nlohmann::json;

namespace {
Config active_compaction_config(const niminal::Agent& agent, Config cfg) {
  if (agent.provider == "local") {
    for (const auto& model : load_local_models()) {
      if (model.model == agent.model && model.api_url == agent.api_url) {
        cfg.context_window = model.context_window;
        cfg.reserve_tokens = std::min(cfg.reserve_tokens, model.context_window / 4);
        cfg.keep_recent_tokens = std::min(cfg.keep_recent_tokens, model.context_window / 2);
        break;
      }
    }
  }
  return cfg;
}
} // namespace

int estimate_tokens(std::string_view text) {
  if (text.empty()) {
    return 1;
  }
  return static_cast<int>((text.size() + 3) / 4);
}

int estimate_session_tokens(const Session& session) {
  int n = 0;
  const int compact = session.latest_compaction_index();
  if (compact >= 0) {
    n += estimate_tokens(session.events[static_cast<size_t>(compact)].value("summary", ""));
  }
  const size_t start =
      compact < 0
          ? 0
          : static_cast<size_t>(std::max(
                0, session.events[static_cast<size_t>(compact)].value("first_kept_index", 0)));
  for (size_t i = start; i < session.events.size(); ++i) {
    n += estimate_session_event_tokens(session.events[i]);
  }
  return n;
}

bool should_compact(const Session& session, int context_window, int reserve_tokens) {
  if (context_window <= 0) {
    return false;
  }
  int limit = context_window - std::max(0, reserve_tokens);
  if (limit <= 0) {
    return true;
  }
  return estimate_session_tokens(session) > limit;
}

int find_cut_index(const Session& session, int keep_recent_tokens, int from_index) {
  if (session.events.empty() || from_index >= static_cast<int>(session.events.size())) {
    return -1;
  }
  int tokens = 0;
  int i = static_cast<int>(session.events.size()) - 1;
  while (i >= from_index) {
    tokens += estimate_session_event_tokens(session.events[static_cast<size_t>(i)]);
    if (tokens >= std::max(1, keep_recent_tokens)) {
      int cut = i;
      while (cut > from_index &&
             session.events[static_cast<size_t>(cut)].value("type", "") != "user") {
        --cut;
      }
      if (session.events[static_cast<size_t>(cut)].value("type", "") != "user") {
        return -1;
      }
      if (cut <= from_index) {
        return -1;
      }
      return cut;
    }
    --i;
  }
  return -1;
}

std::string serialize_range(const Session& session, int start, int end) {
  std::ostringstream out;
  int hi = std::min(end, static_cast<int>(session.events.size()));
  for (int i = std::max(0, start); i < hi; ++i) {
    auto chunk = serialize_session_event(session.events[static_cast<size_t>(i)]);
    if (!chunk.empty()) {
      out << chunk << '\n';
    }
  }
  return out.str();
}

const char* kSummarySystem =
    R"(You are compacting a coding-agent session into structured working memory.
Write a concise markdown summary with these sections when relevant:

## Goal
## Current task
## Important decisions
## Files inspected
## Files modified
## Important symbols/locations
## Commands run and results
## Errors/failures
## Outstanding work
## User preferences/instructions
## Next likely steps

Preserve exact code snippets only when materially important.
Do not invent facts. Prefer concrete paths, commands, and outcomes.)";

std::string build_summary_prompt(const std::string& previous, const std::string& conversation,
                                 const std::string& instruction) {
  std::ostringstream out;
  if (!previous.empty()) {
    out << "<previous-summary>\n" << previous << "\n</previous-summary>\n\n";
  }
  if (!instruction.empty()) {
    out << "<compaction-instructions>\n" << instruction << "\n</compaction-instructions>\n\n";
  }
  out << "<conversation>\n" << conversation << "\n</conversation>\n\n";
  out << "Produce the updated structured summary now.";
  return out.str();
}

CompactResult compact_session(Session& session, niminal::Agent& agent,
                              const std::string& requested_instruction,
                              const std::shared_ptr<ExtensionRuntime>& extensions,
                              const Config& cfg) {
  const Config effective = active_compaction_config(agent, cfg);
  CompactResult result;
  result.message = "Nothing to compact (recent history fits in keep window).";
  int tokens_before = estimate_session_tokens(session);
  result.tokens_before = tokens_before;
  std::string instruction = requested_instruction;
  if (extensions) {
    auto pre = extensions->dispatch(HookEvent::session_before_compact,
                                    json{{"session_id", session.id},
                                         {"workspace", session.workspace},
                                         {"instruction", instruction},
                                         {"tokens_before", tokens_before},
                                         {"entries", session.events}});
    result.warnings = pre.warnings;
    if (!pre.allowed) {
      result.message = pre.reason;
      return result;
    }
    if (!pre.instruction.empty()) {
      if (!instruction.empty()) {
        instruction += '\n';
      }
      instruction += pre.instruction;
    }
    if (pre.has_compaction) {
      session.add_compaction(pre.summary, pre.first_kept_index, tokens_before, pre.details);
      result.did = true;
      result.summary = pre.summary;
      result.first_kept_index = pre.first_kept_index;
      result.message =
          "Compacted by extension; kept from event #" + std::to_string(pre.first_kept_index) + ".";
      auto post = extensions->dispatch(HookEvent::session_compact,
                                       json{{"session_id", session.id},
                                            {"workspace", session.workspace},
                                            {"did_compact", true},
                                            {"summary", result.summary},
                                            {"first_kept_index", result.first_kept_index},
                                            {"tokens_before", tokens_before},
                                            {"message", result.message}});
      result.warnings.insert(result.warnings.end(), post.warnings.begin(), post.warnings.end());
      return result;
    }
  }
  int from = 0;
  std::string previous;
  int compact = session.latest_compaction_index();
  if (compact >= 0) {
    previous = session.events[static_cast<size_t>(compact)].value("summary", "");
    from = session.events[static_cast<size_t>(compact)].value("first_kept_index", 0);
  }
  int cut = find_cut_index(session, effective.keep_recent_tokens, from);
  if (cut < 0) {
    if (extensions) {
      auto post =
          extensions->dispatch(HookEvent::session_compact, json{{"session_id", session.id},
                                                                {"workspace", session.workspace},
                                                                {"did_compact", false},
                                                                {"summary", ""},
                                                                {"first_kept_index", 0},
                                                                {"tokens_before", tokens_before},
                                                                {"message", result.message}});
      result.warnings.insert(result.warnings.end(), post.warnings.begin(), post.warnings.end());
    }
    return result;
  }
  auto conversation = serialize_range(session, from, cut);
  if (conversation.find_first_not_of(" \n\t") == std::string::npos) {
    result.message = "Nothing to compact (empty range).";
    if (extensions) {
      auto post =
          extensions->dispatch(HookEvent::session_compact, json{{"session_id", session.id},
                                                                {"workspace", session.workspace},
                                                                {"did_compact", false},
                                                                {"summary", ""},
                                                                {"first_kept_index", 0},
                                                                {"tokens_before", tokens_before},
                                                                {"message", result.message}});
      result.warnings.insert(result.warnings.end(), post.warnings.begin(), post.warnings.end());
    }
    return result;
  }

  json sys = json::object();
  sys["role"] = "system";
  json parts = json::array();
  json part = json::object();
  part["type"] = "text";
  part["text"] = kSummarySystem;
  parts.push_back(std::move(part));
  sys["content"] = std::move(parts);

  json user = json::object();
  user["role"] = "user";
  user["content"] = build_summary_prompt(previous, conversation, instruction);

  niminal::ChatRequest req;
  agent.fill_chat(req);
  req.messages = json::array({std::move(sys), std::move(user)});
  req.max_tokens = kSummaryMaxTokens;

  std::string summary;
  try {
    summary = niminal::complete_chat(req);
  } catch (const std::exception& e) {
    if (extensions) {
      auto failed = extensions->dispatch(HookEvent::session_compact_failed,
                                         json{{"session_id", session.id},
                                              {"workspace", session.workspace},
                                              {"error", e.what()},
                                              {"tokens_before", tokens_before}});
      result.warnings.insert(result.warnings.end(), failed.warnings.begin(), failed.warnings.end());
    }
    throw;
  }
  while (!summary.empty() &&
         (summary.back() == ' ' || summary.back() == '\n' || summary.back() == '\r')) {
    summary.pop_back();
  }
  if (summary.empty()) {
    if (extensions) {
      extensions->dispatch(HookEvent::session_compact_failed,
                           json{{"session_id", session.id},
                                {"workspace", session.workspace},
                                {"error", "compaction produced an empty summary"},
                                {"tokens_before", tokens_before}});
    }
    throw niminal::Error("compaction produced an empty summary");
  }

  session.add_compaction(summary, cut, tokens_before);
  result.did = true;
  result.summary = summary;
  result.first_kept_index = cut;
  result.message = "Compacted up to event #" + std::to_string(cut) + "; kept recent verbatim (" +
                   std::to_string(tokens_before) + " tokens before).";
  if (extensions) {
    auto post =
        extensions->dispatch(HookEvent::session_compact, json{{"session_id", session.id},
                                                              {"workspace", session.workspace},
                                                              {"did_compact", true},
                                                              {"summary", summary},
                                                              {"first_kept_index", cut},
                                                              {"tokens_before", tokens_before},
                                                              {"message", result.message}});
    result.warnings.insert(result.warnings.end(), post.warnings.begin(), post.warnings.end());
  }
  return result;
}

void bind_compaction(niminal::Agent& agent, Session& session,
                     const std::function<void(const std::string&)>& note,
                     const std::shared_ptr<ExtensionRuntime>& extensions, const Config& cfg) {
  agent.before_request = [&agent, &session, note, extensions, cfg] {
    if (!cfg.compaction_enabled) {
      return;
    }
    const Config effective = active_compaction_config(agent, cfg);
    int window = effective.context_window > 0 ? effective.context_window : kDefaultContextWindow;
    if (!should_compact(session, window, effective.reserve_tokens)) {
      return;
    }
    const int compact = session.latest_compaction_index();
    const int from =
        compact < 0 ? 0 : session.events[static_cast<size_t>(compact)].value("first_kept_index", 0);
    if (find_cut_index(session, effective.keep_recent_tokens, from) < 0) {
      return;
    }
    if (note) {
      note("Context is large; compacting…");
    }
    auto result = compact_session(session, agent, {}, extensions, cfg);
    agent.messages = session.openai_messages();
    if (note) {
      for (const auto& warning : result.warnings) {
        note(warning);
      }
    }
    if (note) {
      note(result.message);
    }
  };
  agent.recover_overflow = [&agent, &session, note, extensions, cfg] {
    if (note) {
      note("Context overflow — compacting and retrying…");
    }
    try {
      auto result = compact_session(session, agent, "Prioritize recovering from context overflow.",
                                    extensions, cfg);
      agent.messages = session.openai_messages();
      if (note) {
        for (const auto& warning : result.warnings) {
          note(warning);
        }
      }
      if (note) {
        note(result.message);
      }
      return result.did;
    } catch (const std::exception& e) {
      if (note) {
        note(std::string("Compaction failed: ") + e.what());
      }
      return false;
    }
  };
}

} // namespace niminal::app
