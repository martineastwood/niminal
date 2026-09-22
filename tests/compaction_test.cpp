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

  s.add_compaction("summary of early turns", cut, 99);
  auto msgs = s.openai_messages();
  if (msgs.empty() ||
      msgs[0].value("content", std::string{}).find("summary of early turns") == std::string::npos) {
    return fail("openai_messages starts with compaction summary");
  }

  std::filesystem::remove_all(dir);
  return 0;
}
