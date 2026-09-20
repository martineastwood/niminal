#pragma once

#include "session.hpp"

#include <niminal/agent.hpp>

#include <functional>
#include <memory>
#include <string>
#include <string_view>

namespace niminal::app {

class ExtensionRuntime;

constexpr int kContextWindow = 128'000;
constexpr int kReserveTokens = 16'384;
constexpr int kKeepRecentTokens = 20'000;

int estimate_tokens(std::string_view text);
int estimate_session_tokens(const Session& session);
int find_cut_index(const Session& session, int keep_recent_tokens,
                   int from_index = 0);
bool should_compact(const Session& session, int context_window = kContextWindow,
                    int reserve_tokens = kReserveTokens);

struct CompactResult {
  bool did = false;
  std::string message;
  std::string summary;
  int first_kept_index = 0;
  int tokens_before = 0;
  std::vector<std::string> warnings;
};

CompactResult compact_session(Session& session, niminal::Agent& agent,
                              const std::string& instruction = {},
                              const std::shared_ptr<ExtensionRuntime>& extensions = {});
void bind_compaction(niminal::Agent& agent, Session& session,
                     std::function<void(const std::string&)> note = {},
                     const std::shared_ptr<ExtensionRuntime>& extensions = {});

}  // namespace niminal::app
