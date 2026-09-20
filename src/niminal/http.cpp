#include <niminal/http.hpp>
#include <niminal/types.hpp>

#include <curl/curl.h>

#include <atomic>
#include <cstdlib>
#include <functional>
#include <mutex>
#include <string>
#include <unistd.h>
#include <utility>

namespace niminal {
namespace {

std::once_flag curl_once;

void ensure_curl() {
  std::call_once(curl_once, [] {
    if (curl_global_init(CURL_GLOBAL_DEFAULT) != 0)
      throw Error("curl_global_init failed");
  });
}

struct WriteBuf {
  std::string* body = nullptr;
  std::string pending;
  std::function<void(std::string_view)>* on_data = nullptr;
  std::atomic<bool>* cancel = nullptr;
};

size_t write_body(char* ptr, size_t size, size_t nmemb, void* userdata) {
  auto* buf = static_cast<WriteBuf*>(userdata);
  buf->body->append(ptr, size * nmemb);
  return size * nmemb;
}

void flush_sse_line(WriteBuf& buf, std::string_view line) {
  if (line.ends_with('\r')) line.remove_suffix(1);
  if (!line.starts_with("data:")) return;
  auto data = line.substr(5);
  while (!data.empty() && (data.front() == ' ' || data.front() == '\t'))
    data.remove_prefix(1);
  if (data.empty() || data == "[DONE]") return;
  if (buf.on_data) (*buf.on_data)(data);
}

size_t write_sse(char* ptr, size_t size, size_t nmemb, void* userdata) {
  auto* buf = static_cast<WriteBuf*>(userdata);
  if (buf->cancel && buf->cancel->load()) return 0;
  buf->pending.append(ptr, size * nmemb);
  if (buf->body) buf->body->append(ptr, size * nmemb);
  size_t start = 0;
  while (start < buf->pending.size()) {
    auto nl = buf->pending.find('\n', start);
    if (nl == std::string::npos) break;
    flush_sse_line(*buf, std::string_view(buf->pending).substr(start, nl - start));
    start = nl + 1;
  }
  buf->pending.erase(0, start);
  return size * nmemb;
}

curl_slist* slist_from(const std::map<std::string, std::string>& headers) {
  curl_slist* list = nullptr;
  for (const auto& [k, v] : headers)
    list = curl_slist_append(list, (k + ": " + v).c_str());
  return list;
}

}  // namespace

struct HttpClient::Impl {
  CURL* easy = nullptr;
  Impl() {
    ensure_curl();
    easy = curl_easy_init();
    if (!easy) throw Error("curl_easy_init failed");
  }
  ~Impl() {
    if (easy) curl_easy_cleanup(easy);
  }
};

HttpClient::HttpClient() : impl_(new Impl) {}
HttpClient::~HttpClient() { delete impl_; }

namespace {

std::string default_ca_file() {
  if (const char* env = std::getenv("SSL_CERT_FILE"); env && *env) return env;
  const char* candidates[] = {
      "/opt/homebrew/etc/openssl@3/cert.pem",
      "/usr/local/etc/openssl@3/cert.pem",
      "/etc/ssl/cert.pem",
      "/etc/ssl/certs/ca-certificates.crt",
  };
  for (auto path : candidates) {
    if (access(path, R_OK) == 0) return path;
  }
  return {};
}

int xfer_progress(void* clientp,
                  curl_off_t, curl_off_t, curl_off_t, curl_off_t) {
  auto* cancel = static_cast<std::atomic<bool>*>(clientp);
  if (cancel && cancel->load()) return 1;
  return 0;
}

void apply_common(CURL* easy, const std::string& url, curl_slist* hdrs,
                  const std::string& ca) {
  curl_easy_setopt(easy, CURLOPT_URL, url.c_str());
  curl_easy_setopt(easy, CURLOPT_HTTPHEADER, hdrs);
  curl_easy_setopt(easy, CURLOPT_FOLLOWLOCATION, 1L);
  curl_easy_setopt(easy, CURLOPT_USERAGENT, "niminal/0.1");
  if (!ca.empty()) curl_easy_setopt(easy, CURLOPT_CAINFO, ca.c_str());
}

void apply_post(CURL* easy, const std::string& body) {
  curl_easy_setopt(easy, CURLOPT_POST, 1L);
  curl_easy_setopt(easy, CURLOPT_POSTFIELDS, body.c_str());
  curl_easy_setopt(easy, CURLOPT_POSTFIELDSIZE, static_cast<long>(body.size()));
}

}  // namespace

HttpResponse HttpClient::post(std::string_view url,
                              const std::map<std::string, std::string>& headers,
                              std::string_view body) {
  HttpResponse out;
  WriteBuf buf{&out.body, {}, nullptr, nullptr};
  auto* hdrs = slist_from(headers);
  std::string url_owned(url);
  std::string body_owned(body);
  std::string ca = default_ca_file();
  curl_easy_reset(impl_->easy);
  apply_common(impl_->easy, url_owned, hdrs, ca);
  apply_post(impl_->easy, body_owned);
  curl_easy_setopt(impl_->easy, CURLOPT_WRITEFUNCTION, write_body);
  curl_easy_setopt(impl_->easy, CURLOPT_WRITEDATA, &buf);
  const auto rc = curl_easy_perform(impl_->easy);
  curl_easy_getinfo(impl_->easy, CURLINFO_RESPONSE_CODE, &out.status);
  curl_slist_free_all(hdrs);
  if (rc != CURLE_OK)
    throw Error(std::string("http: ") + curl_easy_strerror(rc));
  return out;
}

void HttpClient::post_sse(
    std::string_view url, const std::map<std::string, std::string>& headers,
    std::string_view body,
    const std::function<void(std::string_view json_data)>& on_data,
    std::atomic<bool>* cancel) {
  auto on_data_mut = on_data;
  std::string raw;
  WriteBuf buf{&raw, {}, &on_data_mut, cancel};
  auto* hdrs = slist_from(headers);
  std::string url_owned(url);
  std::string body_owned(body);
  std::string ca = default_ca_file();
  curl_easy_reset(impl_->easy);
  apply_common(impl_->easy, url_owned, hdrs, ca);
  apply_post(impl_->easy, body_owned);
  curl_easy_setopt(impl_->easy, CURLOPT_WRITEFUNCTION, write_sse);
  curl_easy_setopt(impl_->easy, CURLOPT_WRITEDATA, &buf);
  curl_easy_setopt(impl_->easy, CURLOPT_TCP_NODELAY, 1L);
  curl_easy_setopt(impl_->easy, CURLOPT_BUFFERSIZE, 1024L);
  curl_easy_setopt(impl_->easy, CURLOPT_HTTP_VERSION, CURL_HTTP_VERSION_1_1);
  curl_easy_setopt(impl_->easy, CURLOPT_NOPROGRESS, 0L);
  curl_easy_setopt(impl_->easy, CURLOPT_XFERINFOFUNCTION, xfer_progress);
  curl_easy_setopt(impl_->easy, CURLOPT_XFERINFODATA, cancel);
  const auto rc = curl_easy_perform(impl_->easy);
  long status = 0;
  curl_easy_getinfo(impl_->easy, CURLINFO_RESPONSE_CODE, &status);
  curl_slist_free_all(hdrs);
  if (cancel && cancel->load() &&
      (rc == CURLE_ABORTED_BY_CALLBACK || rc == CURLE_WRITE_ERROR))
    throw Cancelled();
  if (rc != CURLE_OK)
    throw Error(std::string("http: ") + curl_easy_strerror(rc));
  if (status >= 400) {
    throw Error("http " + std::to_string(status) + ": " + raw);
  }
}

HttpResponse HttpClient::get(std::string_view url, long timeout_seconds) {
  HttpResponse out;
  WriteBuf buf{&out.body, {}, nullptr, nullptr};
  auto* hdrs = slist_from({});
  std::string url_owned(url);
  std::string ca = default_ca_file();
  curl_easy_reset(impl_->easy);
  apply_common(impl_->easy, url_owned, hdrs, ca);
  curl_easy_setopt(impl_->easy, CURLOPT_HTTPGET, 1L);
  curl_easy_setopt(impl_->easy, CURLOPT_TIMEOUT, timeout_seconds);
  curl_easy_setopt(impl_->easy, CURLOPT_WRITEFUNCTION, write_body);
  curl_easy_setopt(impl_->easy, CURLOPT_WRITEDATA, &buf);
  const auto rc = curl_easy_perform(impl_->easy);
  curl_easy_getinfo(impl_->easy, CURLINFO_RESPONSE_CODE, &out.status);
  curl_slist_free_all(hdrs);
  if (rc != CURLE_OK)
    throw Error(std::string("http: ") + curl_easy_strerror(rc));
  return out;
}

}  // namespace niminal
