#include "queue_mode.hpp"
#include "rpc.hpp"

#include <chrono>
#include <csignal>
#include <cstdlib>
#include <fcntl.h>
#include <filesystem>
#include <iostream>
#include <string>
#include <sys/wait.h>
#include <unistd.h>

namespace {

int fail(const char* message) {
  std::cerr << message << '\n';
  return 1;
}

// Runs `<binary> --mode rpc` with stdin pointing at /dev/null. On macOS poll()
// reports POLLNVAL for such a descriptor, and the runtime used to busy-spin at
// 100% CPU instead of exiting. Returns the child's exit status, or -1 if it had
// to be killed for exceeding the deadline.
int run_rpc_with_unpollable_stdin(const std::string& binary, const std::string& home) {
  const pid_t pid = ::fork();
  if (pid < 0) {
    return -1;
  }
  if (pid == 0) {
    const int devnull = ::open("/dev/null", O_RDONLY);
    if (devnull >= 0) {
      ::dup2(devnull, STDIN_FILENO);
    }
    ::setenv("HOME", home.c_str(), 1);
    const char* argv[] = {binary.c_str(), "--mode", "rpc", nullptr};
    ::execv(binary.c_str(), const_cast<char**>(argv));
    ::_exit(127);
  }

  int status = 0;
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
  while (std::chrono::steady_clock::now() < deadline) {
    const pid_t done = ::waitpid(pid, &status, WNOHANG);
    if (done == pid) {
      return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
    }
    ::usleep(20000);
  }
  ::kill(pid, SIGKILL);
  ::waitpid(pid, &status, 0);
  return -1;
}

int check_exits_with_unpollable_stdin(const std::string& binary) {
  const auto home =
      std::filesystem::temp_directory_path() / ("niminal-rpc-" + std::to_string(::getpid()));
  std::filesystem::create_directories(home);
  const int status = run_rpc_with_unpollable_stdin(binary, home.string());
  std::filesystem::remove_all(home);
  if (status != 0) {
    return fail("rpc mode spun on /dev/null stdin instead of exiting");
  }
  return 0;
}

} // namespace

int main(int argc, char** argv) {
  if (!niminal::app::valid_queue_mode("all")) {
    return fail("all is valid");
  }
  if (!niminal::app::valid_queue_mode("one-at-a-time")) {
    return fail("one-at-a-time is valid");
  }
  if (niminal::app::valid_queue_mode("parallel")) {
    return fail("parallel is invalid");
  }

  auto ok = niminal::app::rpc_response_event("req-1", true, "started");
  if (niminal::json_value(ok, "version", 0) != 1 ||
      niminal::json_value(ok, "type", "") != "response" ||
      niminal::json_value(ok, "id", "") != "req-1" || !niminal::json_value(ok, "ok", false) ||
      niminal::json_value(ok, "state", "") != "started") {
    return fail("started response shape");
  }

  auto err = niminal::app::rpc_response_event("req-2", false, {}, "bad command");
  if (niminal::json_value(err, "ok", true) ||
      niminal::json_value(err, "error", "") != "bad command") {
    return fail("error response shape");
  }

  if (argc >= 2 && check_exits_with_unpollable_stdin(argv[1]) != 0) {
    return 1;
  }
  return 0;
}
