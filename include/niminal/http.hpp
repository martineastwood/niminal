#pragma once

#include <atomic>
#include <functional>
#include <map>
#include <string>
#include <string_view>

namespace niminal {

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

  HttpResponse get(std::string_view url, long timeout_seconds = 20);

  HttpResponse post(std::string_view url,
                    const std::map<std::string, std::string>& headers,
                    std::string_view body);

  void post_sse(std::string_view url,
                const std::map<std::string, std::string>& headers,
                std::string_view body,
                const std::function<void(std::string_view json_data)>& on_data,
                std::atomic<bool>* cancel = nullptr);

 private:
  struct Impl;
  Impl* impl_;
};

}  // namespace niminal
