// NIMINAL_API_URL is a startup override, so it must outrank the endpoint that
// provider selection and the config files supply for the process.
#include <arpa/inet.h>
#include <atomic>
#include <chrono>
#include <csignal>
#include <fcntl.h>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <netinet/in.h>
#include <string>
#include <sys/socket.h>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>

namespace {

int fail(const std::string& message) {
  std::cerr << message << '\n';
  return 1;
}

// Accepts connections and answers with an error, counting every client so the
// test can tell which endpoint the process decided to use.
class Endpoint {
public:
  Endpoint() {
    socket_ = ::socket(AF_INET, SOCK_STREAM, 0);
    if (socket_ < 0) {
      return;
    }
    sockaddr_in address{};
    address.sin_family = AF_INET;
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    address.sin_port = 0;
    if (::bind(socket_, reinterpret_cast<sockaddr*>(&address), sizeof(address)) != 0 ||
        ::listen(socket_, 4) != 0) {
      ::close(socket_);
      socket_ = -1;
      return;
    }
    socklen_t size = sizeof(address);
    if (::getsockname(socket_, reinterpret_cast<sockaddr*>(&address), &size) != 0) {
      ::close(socket_);
      socket_ = -1;
      return;
    }
    port_ = ntohs(address.sin_port);
    thread_ = std::thread([this] { serve(); });
  }

  ~Endpoint() {
    if (socket_ >= 0) {
      ::shutdown(socket_, SHUT_RDWR);
      ::close(socket_);
    }
    if (thread_.joinable()) {
      thread_.join();
    }
  }

  Endpoint(const Endpoint&) = delete;
  Endpoint& operator=(const Endpoint&) = delete;

  [[nodiscard]] bool ok() const { return socket_ >= 0; }
  [[nodiscard]] int port() const { return port_; }
  [[nodiscard]] int connections() const { return connections_.load(); }
  [[nodiscard]] std::string url() const {
    return "http://127.0.0.1:" + std::to_string(port_) + "/v1";
  }

private:
  void serve() {
    while (true) {
      const int client = ::accept(socket_, nullptr, nullptr);
      if (client < 0) {
        return;
      }
      connections_.fetch_add(1);
      char buffer[4096];
      ::recv(client, buffer, sizeof(buffer), 0);
      const std::string body = R"({"error":{"message":"endpoint reached"}})";
      const std::string response =
          std::string("HTTP/1.1 400 Bad Request\r\n") +
          "Content-Type: application/json\r\nContent-Length: " + std::to_string(body.size()) +
          "\r\nConnection: close\r\n\r\n" + body;
      ::send(client, response.data(), response.size(), 0);
      ::close(client);
    }
  }

  int socket_ = -1;
  int port_ = 0;
  std::atomic<int> connections_{0};
  std::thread thread_;
};

void write_file(const std::filesystem::path& path, const std::string& text) {
  std::filesystem::create_directories(path.parent_path());
  std::ofstream out(path);
  out << text;
}

// Returns the exit status, or -1 when the child had to be killed.
int run_cli(const std::string& binary, const std::filesystem::path& home,
            const std::string& api_url, const std::filesystem::path& log) {
  const pid_t pid = ::fork();
  if (pid < 0) {
    return -1;
  }
  if (pid == 0) {
    ::setenv("HOME", home.c_str(), 1);
    ::setenv("NIMINAL_API_URL", api_url.c_str(), 1);
    const int out = ::open(log.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (out >= 0) {
      ::dup2(out, STDOUT_FILENO);
      ::dup2(out, STDERR_FILENO);
    }
    const char* argv[] = {binary.c_str(), "--provider", "openrouter",   "--api-key", "test",
                          "--mode",       "json",       "--no-session", "say hello", nullptr};
    ::execv(binary.c_str(), const_cast<char**>(argv));
    ::_exit(127);
  }

  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(60);
  while (std::chrono::steady_clock::now() < deadline) {
    int status = 0;
    if (::waitpid(pid, &status, WNOHANG) == pid) {
      return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
    }
    ::usleep(20000);
  }
  ::kill(pid, SIGKILL);
  ::waitpid(pid, nullptr, 0);
  return -1;
}

} // namespace

int main(int argc, char** argv) {
  if (argc < 2) {
    return fail("usage: niminal_provider_url_test <niminal-binary>");
  }
  const std::string binary = argv[1];

  Endpoint override_endpoint;
  if (!override_endpoint.ok()) {
    return fail("could not listen on a loopback port");
  }

  const auto home = std::filesystem::temp_directory_path() /
                    ("niminal-provider-url-" + std::to_string(::getpid()));
  std::filesystem::remove_all(home);
  std::filesystem::create_directories(home / ".niminal");
  // A cached catalog keeps startup offline; the test is about the endpoint.
  write_file(home / ".niminal" / "models-dev.json",
             R"({"openrouter":{"npm":"@openrouter/ai-sdk-provider",)"
             R"("models":{"mock-model":{"limit":{"context":128000}}}}})");
  const auto log = home / "run.log";

  const int status = run_cli(binary, home, override_endpoint.url(), log);
  if (status == -1) {
    return fail("niminal did not exit within 60s");
  }
  if (override_endpoint.connections() == 0) {
    return fail("NIMINAL_API_URL was not used: --provider discarded the startup override");
  }

  std::ifstream run(log);
  const std::string events((std::istreambuf_iterator<char>(run)), std::istreambuf_iterator<char>());
  if (events.find("\"type\":\"error\"") == std::string::npos) {
    return fail("expected an error event from the overridden endpoint, got: " + events);
  }

  std::filesystem::remove_all(home);
  std::cout << "provider url override reached " << override_endpoint.url() << " (exit " << status
            << ")\n";
  return 0;
}
