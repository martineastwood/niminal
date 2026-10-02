#include "http.hpp"
#include <cail/detail/system_ca.hpp>

#include <glaze/net/http_client.hpp>

#include <chrono>
#include <future>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <system_error>
#include <utility>

namespace niminal::app {
namespace {

struct StreamState {
  HttpResponse response;
  std::optional<std::error_code> error;
  std::promise<void> finished;
  std::once_flag once;
};

} // namespace

struct HttpClient::Impl {
  glz::http_client client;

  Impl() {
    client.set_graceful_ssl_shutdown(false);
    if (const auto trust = cail::detail::configure_system_ca(client); !trust) {
      throw std::system_error(trust.error(), "Could not load trusted TLS certificates");
    }
  }
};

HttpClient::HttpClient() : impl_(std::make_unique<Impl>()) {}
HttpClient::~HttpClient() = default;

Result<HttpResponse> HttpClient::get(std::string_view url, long timeout_seconds) {
  auto state = std::make_shared<StreamState>();
  auto finished = state->finished.get_future();
  const auto timeout = std::chrono::seconds(timeout_seconds);
  glz::stream_request_params_v2 params{
      .method = "GET",
      .url = std::string(url),
      .timeout = timeout,
      .strategy = glz::stream_read_strategy::immediate_delivery,
      .body = {},
      .headers = {},
      .on_data = [state](std::string_view bytes) { state->response.body.append(bytes); },
      .on_error = [state](std::error_code error) { state->error = error; },
      .on_progress = {},
      .on_connect =
          [state](const glz::response& response) { state->response.status = response.status_code; },
      .on_disconnect =
          [state] { std::call_once(state->once, [state] { state->finished.set_value(); }); },
      .status_is_error = [](int) { return false; },
  };
  auto connection = impl_->client.stream_request_v2(params);
  if (!connection) {
    return std::unexpected(Error("http: request could not be started"));
  }
  // Glaze bounds the connect and handshake with the same timeout; this covers a
  // response that stalls after the headers.
  if (finished.wait_for(timeout) == std::future_status::timeout) {
    connection->disconnect();
    finished.wait();
    return std::unexpected(Error("http: request timed out"));
  }
  if (state->error) {
    return std::unexpected(Error("http: " + state->error->message()));
  }
  return state->response;
}

} // namespace niminal::app
