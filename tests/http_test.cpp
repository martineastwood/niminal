#include <niminal/http.hpp>
#include <niminal/openai.hpp>

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
    const std::string body = R"({"choices":[{"message":{"content":"pong"}}]})";
    const std::string response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
                                 "Content-Length: " +
                                 std::to_string(body.size()) + "\r\nConnection: close\r\n\r\n" +
                                 body;
    send(client, response.data(), response.size(), 0);
    close(client);
  });
  niminal::ChatRequest request;
  request.api_url =
      "http://127.0.0.1:" + std::to_string(ntohs(address.sin_port)) + "/v1/chat/completions";
  request.api_key = "test";
  request.model = "original";
  request.messages = nlohmann::json::array({{{"role", "user"}, {"content", "ping"}}});
  request.before_provider_request = [](nlohmann::json& payload) {
    payload["model"] = "replacement";
  };
  request.before_provider_headers = [](std::map<std::string, std::string>& headers) {
    headers.erase("Authorization");
    headers["x-test"] = "yes";
  };
  niminal::HttpResponse observed;
  request.after_provider_response = [&](const niminal::HttpResponse& response) {
    observed = response;
  };
  const auto reply = niminal::complete_chat(request);
  peer.join();
  close(server);
  if (reply != "pong" || observed.status != 200 || observed.duration_ms < 0 ||
      !observed.headers.contains("Content-Type") ||
      received.find("x-test: yes") == std::string::npos ||
      received.find("Authorization:") != std::string::npos ||
      received.find("\"model\":\"replacement\"") == std::string::npos) {
    return fail("provider hooks were not applied to HTTP request");
  }
  return 0;
}
