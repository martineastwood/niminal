#include "http.hpp"
#include <niminal/types.hpp>

#include <curl/curl.h>

#include <cstdlib>
#include <mutex>
#include <string>
#include <unistd.h>
#include <utility>

namespace niminal::app {
namespace {

std::once_flag curl_once;

void ensure_curl() {
  std::call_once(curl_once, [] {
    if (curl_global_init(CURL_GLOBAL_DEFAULT) != 0) {
      throw Error("curl_global_init failed");
    }
  });
}

size_t write_body(char* ptr, size_t size, size_t nmemb, void* userdata) {
  auto& body = *static_cast<std::string*>(userdata);
  body.append(ptr, size * nmemb);
  return size * nmemb;
}

} // namespace

struct HttpClient::Impl {
  CURL* easy = nullptr;
  Impl() {
    ensure_curl();
    easy = curl_easy_init();
    if (easy == nullptr) {
      throw Error("curl_easy_init failed");
    }
  }
  ~Impl() {
    if (easy != nullptr) {
      curl_easy_cleanup(easy);
    }
  }
};

HttpClient::HttpClient() : impl_(std::make_unique<Impl>()) {}
HttpClient::~HttpClient() = default;

namespace {

std::string default_ca_file() {
  if (const char* env = std::getenv("SSL_CERT_FILE"); (env != nullptr) && ((*env) != 0)) {
    return env;
  }
  const char* candidates[] = {
      "/opt/homebrew/etc/openssl@3/cert.pem",
      "/usr/local/etc/openssl@3/cert.pem",
      "/etc/ssl/cert.pem",
      "/etc/ssl/certs/ca-certificates.crt",
  };
  for (auto path : candidates) {
    if (access(path, R_OK) == 0) {
      return path;
    }
  }
  return {};
}

void apply_common(CURL* easy, const std::string& url, const std::string& ca) {
  curl_easy_setopt(easy, CURLOPT_URL, url.c_str());
  curl_easy_setopt(easy, CURLOPT_FOLLOWLOCATION, 1L);
  curl_easy_setopt(easy, CURLOPT_USERAGENT, "niminal/0.1");
  if (!ca.empty()) {
    curl_easy_setopt(easy, CURLOPT_CAINFO, ca.c_str());
  }
}

} // namespace

Result<HttpResponse> HttpClient::get(std::string_view url, long timeout_seconds) {
  HttpResponse out;
  std::string url_owned(url);
  std::string ca = default_ca_file();
  curl_easy_reset(impl_->easy);
  apply_common(impl_->easy, url_owned, ca);
  curl_easy_setopt(impl_->easy, CURLOPT_HTTPGET, 1L);
  curl_easy_setopt(impl_->easy, CURLOPT_TIMEOUT, timeout_seconds);
  curl_easy_setopt(impl_->easy, CURLOPT_WRITEFUNCTION, write_body);
  curl_easy_setopt(impl_->easy, CURLOPT_WRITEDATA, &out.body);
  const auto rc = curl_easy_perform(impl_->easy);
  curl_easy_getinfo(impl_->easy, CURLINFO_RESPONSE_CODE, &out.status);
  if (rc != CURLE_OK) {
    return std::unexpected(Error(std::string("http: ") + curl_easy_strerror(rc)));
  }
  return out;
}

} // namespace niminal::app
