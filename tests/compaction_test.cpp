#include "compaction.hpp"
#include "session.hpp"

#include <filesystem>
#include <iostream>
#include <string>

using niminal::app::create_session;
using niminal::app::estimate_tokens;
using niminal::app::find_cut_index;
using niminal::app::Session;
using niminal::app::should_compact;
using json = nlohmann::json;

static int fail(const char* msg) {
  std::cerr << msg << '\n';
  return 1;
}

int main() {
  if (estimate_tokens("abcd") != 1) {
    return fail("estimate_tokens 4 chars");
  }
  if (estimate_tokens("abcdefgh") != 2) {
    return fail("estimate_tokens 8 chars");
  }

  auto dir = std::filesystem::temp_directory_path() / "niminal-compaction-test";
  std::filesystem::remove_all(dir);
  std::filesystem::create_directories(dir);
  auto s = create_session(dir, "/tmp/ws");
  s.persist = false;
  s.path.clear();

  std::string blob(80, 'x');
  for (int i = 0; i < 6; ++i) {
    s.add_user("turn " + std::to_string(i) + " " + blob);
    s.add_assistant("ok " + std::to_string(i) + " " + blob, json::array(), "m");
  }
  int cut = find_cut_index(s, 40, 0);
  if (cut < 2) {
    return fail("cut should leave older events behind");
  }
  if (s.events[static_cast<size_t>(cut)].value("type", "") != "user") {
    return fail("cut lands on a user event");
  }

  if (!should_compact(s, 50, 0)) {
    return fail("should_compact when over limit");
  }
  if (should_compact(s, 1'000'000, 0)) {
    return fail("should_compact when under limit");
  }

  auto image_session = create_session(dir, "/tmp/ws");
  image_session.persist = false;
  image_session.add_user(
      niminal::UserInput{"look", json::array({{{"type", "image"},
                                               {"name", "screen.png"},
                                               {"mime_type", "image/png"},
                                               {"data", std::string(10000, 'x')}}})});
  if (!should_compact(image_session, 900, 0) || should_compact(image_session, 1200, 0)) {
    return fail("image token estimate should not count base64 bytes");
  }

  auto large_result = create_session(dir, "/tmp/ws");
  large_result.persist = false;
  large_result.add_user("search the workspace");
  large_result.add_tool_result("grep-1", std::string(424'000, 'x'), false);
  if (should_compact(large_result, 128'000, 16'384) ||
      find_cut_index(large_result, 20'000, 0) != -1) {
    return fail("one large tool result should not trigger impossible compaction");
  }
  auto bounded = large_result.openai_messages();
  const auto context = bounded.back().value("content", std::string{});
  if (context.size() > 8'100 || context.find("[truncated]") == std::string::npos ||
      large_result.events.back().value("output", std::string{}).size() != 424'000) {
    return fail("tool context should be bounded while session keeps full result");
  }
  niminal::Agent one_turn_agent;
  niminal::app::Config small_context;
  small_context.context_window = 100;
  small_context.reserve_tokens = 0;
  small_context.keep_recent_tokens = 40;
  int notices = 0;
  niminal::app::bind_compaction(one_turn_agent, large_result, small_context,
                                [&](const std::string&) { ++notices; });
  one_turn_agent.before_request();
  if (notices != 0) {
    return fail("no compaction notice when no older turn can be removed");
  }

  niminal::Agent configured_agent;
  int summaries = 0;
  configured_agent.language_model = cail::LanguageModel{
      [&](const cail::GenerationRequest&) -> cail::Result<cail::GenerationResponse> {
        ++summaries;
        return cail::GenerationResponse{.text = "Earlier turns summarized."};
      }};
  small_context.context_window = 1'000'000;
  niminal::app::bind_compaction(configured_agent, s, small_context);
  configured_agent.before_request();
  small_context.context_window = 100;
  configured_agent.before_request();
  if (summaries != 1 || s.latest_compaction_index() < 0) {
    return fail("compaction should use the current configuration and configured Cail model");
  }

  std::filesystem::remove_all(dir);
  return 0;
}
