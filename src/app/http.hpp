#pragma once

#include <niminal/types.hpp>

#include <memory>
#include <string>
#include <string_view>

namespace niminal::app {

struct HttpResponse {
  long status = 0;
  std::string body;
};

class HttpClient {
public:
  HttpClient();
  ~HttpClient();
  HttpClient(const HttpClient&) = delete;
  HttpClient& operator=(const HttpClient&) = delete;

  Result<HttpResponse> get(std::string_view url, long timeout_seconds = 20);

private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
};

} // namespace niminal::app
