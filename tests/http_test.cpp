#include <niminal/http.hpp>

#include <iostream>
#include <string>

namespace {

int fail(const char* message) {
  std::cerr << message << '\n';
  return 1;
}

} // namespace

int main() {
  niminal::HttpClient http;
  auto refused = http.get("http://127.0.0.1:1", 1);
  if (refused) {
    return fail("connection refused should return an error");
  }
  if (std::string(refused.error().what()).find("http:") == std::string::npos) {
    return fail("http error should mention http:");
  }

  auto bad_scheme = http.get("not-a-valid-url", 1);
  if (bad_scheme) {
    return fail("invalid url should return an error");
  }

  auto posted = http.post("http://127.0.0.1:1", {}, "body");
  if (posted) {
    return fail("post to closed port should return an error");
  }
  return 0;
}
