#include "http.hpp"
#include <cail/foundry.hpp>
#include <cail/local.hpp>
#include <cail/mistral.hpp>
#include <cail/ollama_cloud.hpp>
#include <niminal/chat.hpp>

#include <arpa/inet.h>
#include <iostream>
#include <netinet/in.h>
#include <string>
#include <sys/socket.h>
#include <thread>
#include <unistd.h>

namespace {

int fail(const char* message) {
  std::cerr << message << '\n';
  return 1;
}

} // namespace

int main() {
  niminal::app::HttpClient http;
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

  for (const auto* provider : {"mistral", "local", "ollama", "foundry"}) {
    const int server = socket(AF_INET, SOCK_STREAM, 0);
    if (server < 0) {
      return fail("socket failed");
    }
    sockaddr_in address{};
    address.sin_family = AF_INET;
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    address.sin_port = 0;
    if (bind(server, reinterpret_cast<sockaddr*>(&address), sizeof(address)) != 0 ||
        listen(server, 1) != 0) {
      close(server);
      return fail("listen failed");
    }
    socklen_t address_size = sizeof(address);
    getsockname(server, reinterpret_cast<sockaddr*>(&address), &address_size);
    std::string received;
    std::thread peer([&] {
      const int client = accept(server, nullptr, nullptr);
      if (client < 0) {
        return;
      }
      char buffer[4096];
      while (true) {
        const auto count = recv(client, buffer, sizeof(buffer), 0);
        if (count <= 0) {
          break;
        }
        received.append(buffer, static_cast<size_t>(count));
        const auto end = received.find("\r\n\r\n");
        if (end == std::string::npos) {
          continue;
        }
        const auto length_pos = received.find("Content-Length: ");
        if (length_pos == std::string::npos) {
          continue;
        }
        const auto length_end = received.find("\r\n", length_pos);
        const auto length =
            std::stoul(received.substr(length_pos + 16, length_end - length_pos - 16));
        if (received.size() >= end + 4 + length) {
          break;
        }
      }
      const std::string body =
          std::string_view(provider) == "foundry"
              ? R"({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"pong"}]}]})"
              : R"({"choices":[{"index":0,"finish_reason":"stop","message":{"content":"pong"}}]})";
      const std::string response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
                                   "Content-Length: " +
                                   std::to_string(body.size()) + "\r\nConnection: close\r\n\r\n" +
                                   body;
      send(client, response.data(), response.size(), 0);
      close(client);
    });
    const std::string provider_name = provider;
    std::string api_url =
        "http://127.0.0.1:" + std::to_string(ntohs(address.sin_port)) + "/v1/chat/completions";
    if (provider_name == "foundry") {
      api_url = "http://127.0.0.1:" + std::to_string(ntohs(address.sin_port)) +
                "/openai/responses?api-version=2025-04-01-preview";
    }
    niminal::ChatRequest request;
    if (provider_name == "foundry") {
      request.model = cail::create_foundry({.api_key = "test"})(
          {.endpoint = api_url, .deployment = "original"});
    } else if (provider_name == "local") {
      request.model = cail::create_local({.endpoint = api_url})("original");
    } else if (provider_name == "mistral") {
      request.model = cail::create_mistral({.api_key = "test", .endpoint = api_url})("original");
    } else {
      request.model =
          cail::create_ollama_cloud({.api_key = "test", .endpoint = api_url})("original");
    }
    request.messages = nlohmann::json::array({
        {{"role", "assistant"},
         {"content", "Hello"},
         {"provider_options", {{"reasoning_content", "private thought"}}}},
        {{"role", "user"}, {"content", "ping"}},
    });
    request.before_provider_request = [](nlohmann::json& payload) {
      payload["model"] = "replacement";
    };
    bool authorization_correct = false;
    request.before_provider_headers = [&](std::map<std::string, std::string>& headers) {
      authorization_correct = provider_name == "local" ? !headers.contains("Authorization")
                                                       : headers["Authorization"] == "Bearer test";
      headers.erase("Authorization");
      headers["x-test"] = "yes";
    };
    niminal::ProviderResponse observed;
    request.after_provider_response = [&](const niminal::ProviderResponse& response) {
      observed = response;
    };
    const auto reply = niminal::complete_chat(request);
    peer.join();
    close(server);
    if (provider_name == "foundry" &&
        received.find("POST /openai/responses?api-version=2025-04-01-preview HTTP/1.1") ==
            std::string::npos) {
      return fail("Foundry must preserve the full deployment endpoint and API version");
    }
    const bool echoed = received.find("private thought") != std::string::npos;
    if (reply != "pong" || !authorization_correct || observed.status != 200 ||
        observed.duration_ms < 0 || !observed.headers.contains("Content-Type") ||
        received.find("x-test: yes") == std::string::npos ||
        received.find("Authorization:") != std::string::npos ||
        (provider_name == "mistral" && echoed) ||
        ((provider_name == "local" || provider_name == "ollama") && !echoed) ||
        received.find("\"model\":\"replacement\"") == std::string::npos) {
      return fail("provider hooks were not applied to HTTP request");
    }
  }
  return 0;
}
