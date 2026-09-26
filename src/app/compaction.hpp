#pragma once

#include "config.hpp"
#include "session.hpp"

#include <niminal/agent.hpp>

#include <functional>
#include <memory>
#include <string>
#include <string_view>

namespace niminal::app {

class ExtensionRuntime;

int estimate_tokens(std::string_view text);
int estimate_session_tokens(const Session& session);
int find_cut_index(const Session& session, int keep_recent_tokens, int from_index = 0);
bool should_compact(const Session& session, int context_window = kDefaultContextWindow,
                    int reserve_tokens = kDefaultReserveTokens);

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
                              const std::shared_ptr<ExtensionRuntime>& extensions = {},
                              const Config& cfg = {});
void bind_compaction(niminal::Agent& agent, Session& session, const Config& cfg,
                     const std::function<void(const std::string&)>& note = {},
                     const std::shared_ptr<ExtensionRuntime>& extensions = {});

} // namespace niminal::app
