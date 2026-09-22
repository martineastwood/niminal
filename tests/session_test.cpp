#include "session.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>

namespace fs = std::filesystem;
using niminal::app::create_session;
using niminal::app::delete_session;
using niminal::app::format_session_list;
using niminal::app::list_deleted_sessions;
using niminal::app::list_sessions;
using niminal::app::load_session;
using niminal::app::restore_session;
using niminal::app::search_sessions;
using niminal::app::valid_session_id;
using json = nlohmann::json;

static int fail(const char* msg) {
  std::cerr << msg << '\n';
  return 1;
}

int main() {
  if (valid_session_id("") || valid_session_id("../x") || valid_session_id("id.jsonl") ||
      !valid_session_id("1789233281025102")) {
    return fail("valid_session_id");
  }

  auto dir = fs::temp_directory_path() / "niminal-session-test";
  fs::remove_all(dir);
  fs::create_directories(dir);

  auto s = create_session(dir, "/tmp/ws-a");
  if (s.id.empty() || !valid_session_id(s.id)) {
    return fail("create id");
  }
  if (fs::exists(s.path)) {
    return fail("lazy create wrote a file");
  }

  s.add_user("why is the parser test failing?");
  if (!fs::exists(s.path)) {
    return fail("first event should write the file");
  }

  {
    std::ifstream in(s.path);
    std::string line;
    std::getline(in, line);
    auto header = json::parse(line);
    if (header.value("type", "") != "session" || header.value("workspace", "") != "/tmp/ws-a") {
      return fail("session header");
    }
  }

  json calls = json::array({json{
      {"id", "call_1"},
      {"type", "function"},
      {"function", {{"name", "bash"}, {"arguments", "{\"command\":\"npm test\"}"}}},
  }});
  json reasoning_details = json::array({{{"type", "reasoning.text"}, {"text", "check"}}});
  s.add_assistant("running tests", calls, "openai/gpt-4o-mini", {}, "check", reasoning_details);
  s.add_tool_result("call_1", "exit_code: 1", true);
  s.add_name("fix the parser");
  s.add_selection("anthropic/claude-sonnet-4", "openrouter");
  s.add_extension("fixture", json{{"count", 1}});

  auto loaded = load_session(dir, s.id);
  if (loaded.name != "fix the parser") {
    return fail("name");
  }
  if (loaded.workspace != "/tmp/ws-a") {
    return fail("workspace");
  }
  if (loaded.events.size() != 6) {
    return fail("event count");
  }
  if (loaded.last_model() != "anthropic/claude-sonnet-4") {
    return fail("last_model prefers selection");
  }
  if (loaded.last_provider() != "openrouter") {
    return fail("last_provider");
  }
  auto msgs = loaded.openai_messages();
  if (msgs.size() != 3) {
    return fail("openai_messages should skip name/selection/extension");
  }
  if (msgs[0].value("role", "") != "user") {
    return fail("user role");
  }
  if (msgs[1].value("role", "") != "assistant" || !msgs[1].contains("tool_calls")) {
    return fail("assistant tool_calls");
  }
  if (msgs[1].value("reasoning_content", "") != "check" ||
      msgs[1].value("reasoning_details", json::array()) != reasoning_details) {
    return fail("assistant reasoning survives session reload");
  }
  if (msgs[2].value("role", "") != "tool") {
    return fail("tool role");
  }
  if (loaded.last_assistant_text() != "running tests") {
    return fail("last_assistant_text");
  }

  s.add_assistant("usage turn", json::array(), "openai/gpt-4o-mini",
                  niminal::Usage{100, 20, 80, 0, true});
  auto totals = s.usage_totals();
  if (totals.input_tokens != 100 || totals.output_tokens != 20 || totals.cache_read_tokens != 80) {
    return fail("usage_totals");
  }
  s.events.push_back(json("not-an-object"));
  s.events.push_back(json::array());
  auto still = s.usage_totals();
  if (still.input_tokens != 100 || still.output_tokens != 20) {
    return fail("usage_totals skips non-objects");
  }

  auto other = create_session(dir, "/tmp/ws-b");
  other.add_user("other workspace");
  auto listed = list_sessions(dir, "/tmp/ws-a", 20);
  if (listed.size() != 1 || listed[0].id != s.id) {
    return fail("list filters by workspace");
  }
  if (listed[0].name != "fix the parser") {
    return fail("list name");
  }

  auto pending = create_session(dir, "/tmp/ws-a");
  pending.add_user("run it");
  pending.add_assistant("",
                        json::array({json{
                            {"id", "call_x"},
                            {"function", {{"name", "bash"}, {"arguments", "{}"}}},
                        }}),
                        "openai/gpt-4o-mini");
  auto recovered = load_session(dir, pending.id);
  if (recovered.recover_interrupted_tools() != 1) {
    return fail("recover count");
  }
  if (recovered.recover_interrupted_tools() != 0) {
    return fail("recover is idempotent");
  }
  auto last = recovered.events.back();
  if (last.value("type", "") != "tool_result" || !last.value("is_error", false)) {
    return fail("interrupted tool_result");
  }
  if (last.value("output", "").find("Interrupted before a tool result") == std::string::npos) {
    return fail("interrupted wording");
  }

  auto damaged = create_session(dir, "/tmp/ws-a");
  damaged.add_user("keep me");
  {
    std::ofstream out(damaged.path, std::ios::app);
    out << "{\"type\":\"assistant\",\"role\":\"assistant\",\"content\":[";
  }
  auto repaired = load_session(dir, damaged.id);
  if (repaired.events.size() != 1) {
    return fail("damaged last line ignored");
  }
  repaired.add_user("after recovery");
  auto again = load_session(dir, damaged.id);
  if (again.events.size() != 2) {
    return fail("append after damage");
  }

  auto compact = create_session(dir, "/tmp/ws-a");
  compact.add_user("old question");
  compact.add_assistant("old answer", json::array(), "openai/gpt-4o-mini");
  compact.add_user("new question");
  compact.add_assistant("new answer", json::array(), "openai/gpt-4o-mini");
  compact.add_compaction("Earlier work: old question.", 2, 400);
  if (compact.latest_compaction_index() != 4) {
    return fail("latest_compaction_index");
  }
  auto sliced = compact.openai_messages();
  if (sliced.size() != 3) {
    return fail("compaction prepends summary then kept turns");
  }
  auto summary = sliced[0].value("content", "");
  if (summary.find("<summary>") == std::string::npos ||
      summary.find("old question") == std::string::npos) {
    return fail("compaction summary wrapper");
  }
  if (sliced[1].value("content", "") != "new question") {
    return fail("kept user after cut");
  }
  if (sliced[2].value("content", "") != "new answer") {
    return fail("kept assistant after cut");
  }
  bool found_backup = false;
  for (const auto& entry : fs::directory_iterator(dir)) {
    auto name = entry.path().filename().string();
    if (name.find(damaged.id + ".jsonl.recovery-") == 0) {
      found_backup = true;
    }
  }
  if (!found_backup) {
    return fail("recovery backup");
  }

  auto clash_id = create_session(dir, "/tmp/ws-a");
  clash_id.add_user("keep");
  {
    std::ofstream out(clash_id.path, std::ios::app);
    out << "{\"type\":\"assistant\",\"role\":\"assistant\",\"content\":[";
  }
  auto clash = load_session(dir, clash_id.id);
  {
    std::ofstream out(clash_id.path, std::ios::app);
    out << "MUTATED";
  }
  try {
    clash.add_user("should fail");
    return fail("changed on disk should throw");
  } catch (const std::runtime_error& e) {
    if (std::string(e.what()).find("Session changed on disk") == std::string::npos) {
      return fail("changed on disk message");
    }
  }

  auto mem = create_session(dir, "/tmp/ws-a");
  mem.persist = false;
  mem.path.clear();
  mem.add_user("ephemeral");
  if (!mem.events.empty() && fs::exists(dir / (mem.id + ".jsonl"))) {
    return fail("no-session must not write");
  }

  auto forked = s.fork(dir);
  if (forked.id == s.id || forked.parent != s.id) {
    return fail("fork lineage");
  }
  if (forked.events.size() != s.events.size()) {
    return fail("fork copies events");
  }
  if (load_session(dir, forked.id).parent != s.id) {
    return fail("fork parent persists");
  }
  if (s.fork(dir, 2).events.size() != 2) {
    return fail("fork keeps a prefix");
  }

  auto turns = create_session(dir, "/tmp/ws-a");
  turns.add_user("first question");
  turns.add_assistant("first answer", json::array(), "openai/gpt-4o-mini");
  turns.add_tool_result("call_t", "tool output", false);
  turns.add_user("second question");
  turns.add_assistant("second answer", json::array(), "openai/gpt-4o-mini");
  if (turns.end_after_user_turn(0) != -1 || turns.end_after_user_turn(99) != -1) {
    return fail("end_after_user_turn invalid");
  }
  if (turns.end_after_user_turn(1) != 3) {
    return fail("end_after_user_turn first");
  }
  if (turns.end_after_user_turn(2) != 5) {
    return fail("end_after_user_turn second");
  }
  if (turns.fork(dir, turns.end_after_user_turn(1)).events.size() != 3) {
    return fail("fork through user turn 1");
  }
  if (turns.fork(dir, turns.end_after_user_turn(2)).events.size() != 5) {
    return fail("fork through user turn 2");
  }
  auto previews = turns.user_turn_previews();
  if (previews.size() != 2 || previews[0].first != 1 || previews[1].first != 2 ||
      previews[0].second.find("first question") == std::string::npos ||
      previews[1].second.find("second question") == std::string::npos) {
    return fail("user_turn_previews");
  }

  auto markdown = forked.export_text("md");
  if (markdown.find("# fix the parser") == std::string::npos ||
      markdown.find("why is the parser test failing?") == std::string::npos ||
      markdown.find("forked from: " + s.id) == std::string::npos) {
    return fail("export markdown");
  }
  auto exported = json::parse(forked.export_text("json"));
  if (exported.value("id", "") != forked.id || exported.value("parent", "") != s.id ||
      exported["events"].size() != forked.events.size()) {
    return fail("export json");
  }

  auto html = forked.export_text("html");
  if (html.find("<!DOCTYPE html>") == std::string::npos ||
      html.find("fix the parser") == std::string::npos ||
      html.find("why is the parser test failing?") == std::string::npos ||
      html.find("running tests") == std::string::npos || html.find("bash") == std::string::npos ||
      html.find("exit_code: 1") == std::string::npos ||
      html.find("forked from") == std::string::npos) {
    return fail("export html content");
  }
  auto danger = create_session(dir, "/tmp/ws-a");
  danger.add_user("<script>alert(1)</script>");
  auto danger_html = danger.export_text("html");
  if (danger_html.find("<script>alert") != std::string::npos) {
    return fail("export html escapes user input");
  }
  if (danger_html.find("&lt;script&gt;") == std::string::npos) {
    return fail("export html has escaped tags");
  }
  auto compact_html = compact.export_text("html");
  if (compact_html.find("Earlier work: old question.") == std::string::npos) {
    return fail("export html compaction");
  }

  auto hits = search_sessions(dir, "/tmp/ws-a", "running tests", 20);
  bool found = false;
  for (const auto& info : hits) {
    if (info.id == s.id) {
      found = true;
    }
  }
  if (!found) {
    return fail("search finds assistant text");
  }
  if (!search_sessions(dir, "/tmp/ws-a", "zzz-no-match", 20).empty()) {
    return fail("search misses");
  }
  if (format_session_list(hits, s.id).find(s.id) == std::string::npos) {
    return fail("format_session_list");
  }

  if (!delete_session(dir, other.id)) {
    return fail("delete");
  }
  if (fs::exists(dir / (other.id + ".jsonl"))) {
    return fail("delete removes file");
  }
  if (!fs::exists(dir / ".trash" / (other.id + ".jsonl"))) {
    return fail("delete moves to trash");
  }
  for (const auto& info : list_sessions(dir, "/tmp/ws-b", 20)) {
    if (info.id == other.id) {
      return fail("deleted session hidden from list");
    }
  }
  bool trashed = false;
  for (const auto& info : list_deleted_sessions(dir)) {
    if (info.id == other.id && info.deleted) {
      trashed = true;
    }
  }
  if (!trashed) {
    return fail("list_deleted_sessions");
  }
  if (!restore_session(dir, other.id)) {
    return fail("restore");
  }
  if (!fs::exists(dir / (other.id + ".jsonl"))) {
    return fail("restore returns file");
  }
  if (restore_session(dir, other.id)) {
    return fail("restore twice fails");
  }
  if (delete_session(dir, "../evil")) {
    return fail("delete rejects bad id");
  }

  fs::remove_all(dir);
  return 0;
}
