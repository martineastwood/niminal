#include "extensions.hpp"

#include <niminal/text.hpp>

#include "compaction.hpp"
#include "config.hpp"
#include "session.hpp"
#include "trust.hpp"

#include <niminal/openai.hpp>

#include <algorithm>
#include <cctype>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <csignal>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <fstream>
#include <limits>
#include <map>
#include <mutex>
#include <poll.h>
#include <set>
#include <sstream>
#include <stdexcept>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>
#include <utility>

namespace niminal::app {
namespace fs = std::filesystem;
using json = nlohmann::json;

namespace {

constexpr int kDefaultTimeoutMs = 30'000;
constexpr size_t kMaxLineBytes = 1'000'000;
constexpr int kDefaultExternalTimeoutSeconds = 30;
constexpr size_t kMaxExternalOutputBytes = 100'000;

std::string string_field(const json& value, const char* key) {
  auto it = value.find(key);
  return it != value.end() && it->is_string() ? it->get<std::string>() : std::string();
}

std::vector<std::string> string_array(const json& value, const char* key) {
  std::vector<std::string> out;
  auto it = value.find(key);
  if (it == value.end() || !it->is_array()) {
    return out;
  }
  for (const auto& item : *it) {
    if (item.is_string()) {
      out.push_back(item.get<std::string>());
    }
  }
  return out;
}

bool builtin_tool(std::string name) {
  for (char& c : name) {
    c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
  }
  static const std::set<std::string> names = {"bash", "edit",  "glob", "grep",
                                              "read", "skill", "write"};
  return names.contains(name);
}

std::vector<fs::path> extension_dirs(const fs::path& workspace) {
  std::vector<fs::path> bases;
  try {
    auto global = config_path().parent_path();
    bases.push_back(global.parent_path() / ".agents" / "extensions");
    bases.push_back(global.parent_path() / ".nimlet" / "extensions");
    bases.push_back(global / "extensions");
  } catch (...) {
  }
  if (project_resources_trusted(workspace)) {
    bases.push_back(workspace / ".agents" / "extensions");
    bases.push_back(workspace / ".nimlet" / "extensions");
    bases.push_back(workspace / ".niminal" / "extensions");
  }
  std::vector<fs::path> result;
  std::error_code ec;
  for (const auto& base : bases) {
    if (!fs::is_directory(base, ec)) {
      continue;
    }
    std::vector<fs::path> found;
    for (const auto& entry : fs::directory_iterator(base, ec)) {
      if (entry.is_directory(ec) && fs::is_regular_file(entry.path() / "extension.json", ec)) {
        found.push_back(entry.path());
      }
    }
    std::sort(found.begin(), found.end());
    result.insert(result.end(), found.begin(), found.end());
  }
  return result;
}

std::vector<fs::path> external_tool_dirs(const fs::path& workspace) {
  std::vector<fs::path> bases;
  try {
    auto global = config_path().parent_path();
    bases.push_back(global.parent_path() / ".agents" / "tools");
    bases.push_back(global.parent_path() / ".nimlet" / "tools");
    bases.push_back(config_path().parent_path() / "tools");
  } catch (...) {
  }
  if (project_resources_trusted(workspace)) {
    bases.push_back(workspace / ".agent" / "tools");
    bases.push_back(workspace / ".agents" / "tools");
    bases.push_back(workspace / ".nimlet" / "tools");
    bases.push_back(workspace / ".niminal" / "tools");
  }
  std::vector<fs::path> result;
  std::error_code ec;
  for (const auto& base : bases) {
    if (!fs::is_directory(base, ec)) {
      continue;
    }
    std::vector<fs::path> found;
    for (const auto& entry : fs::directory_iterator(base, ec)) {
      if (entry.is_directory(ec) && fs::is_regular_file(entry.path() / "tool.json", ec)) {
        found.push_back(entry.path());
      }
    }
    std::sort(found.begin(), found.end());
    result.insert(result.end(), found.begin(), found.end());
  }
  return result;
}

struct Manifest {
  std::string name;
  std::vector<std::string> command;
  int timeout_ms = kDefaultTimeoutMs;
};

struct ExternalTool {
  std::string name;
  std::string description;
  std::vector<std::string> command;
  int timeout_seconds = kDefaultExternalTimeoutSeconds;
  json schema;
  fs::path dir;
  bool read_only = false;
};

bool external_read_only_capabilities(const json& doc) {
  auto it = doc.find("capabilities");
  if (it == doc.end() || it->is_null()) {
    return false;
  }
  if (!it->is_array()) {
    throw std::runtime_error("capabilities must be an array");
  }
  bool any = false;
  bool read_only = true;
  for (const auto& item : *it) {
    if (!item.is_string()) {
      throw std::runtime_error("capabilities must contain strings");
    }
    const auto value = niminal::lower_copy(item.get<std::string>());
    if (value != "read" && value != "write" && value != "shell" && value != "network" &&
        value != "user") {
      throw std::runtime_error("unknown capability: " + value);
    }
    any = true;
    read_only = read_only && value == "read";
  }
  return any && read_only;
}

ExternalTool parse_external_manifest(const fs::path& path) {
  std::ifstream in(path);
  if (!in) {
    throw std::runtime_error("cannot read manifest");
  }
  json doc;
  try {
    in >> doc;
  } catch (const std::exception& e) {
    throw std::runtime_error("invalid JSON: " + std::string(e.what()));
  }
  if (!doc.is_object()) {
    throw std::runtime_error("manifest must be an object");
  }

  ExternalTool out;
  out.name = string_field(doc, "name");
  out.description = string_field(doc, "description");
  if (out.name.empty()) {
    throw std::runtime_error("missing name");
  }
  if (out.description.empty()) {
    throw std::runtime_error("missing description");
  }

  auto command = doc.find("command");
  if (command == doc.end() || !command->is_array() || command->empty()) {
    throw std::runtime_error("command must be a nonempty array");
  }
  for (const auto& item : *command) {
    if (!item.is_string() || item.get<std::string>().empty()) {
      throw std::runtime_error("command entries must be nonempty strings");
    }
    out.command.push_back(item.get<std::string>());
  }

  auto schema = doc.find("input_schema");
  if (schema == doc.end() || !schema->is_object()) {
    throw std::runtime_error("input_schema must be a JSON object");
  }
  out.schema = *schema;

  if (auto timeout = doc.find("timeout_seconds"); timeout != doc.end()) {
    if (!timeout->is_number_integer()) {
      throw std::runtime_error("timeout_seconds must be an integer");
    }
    const auto seconds = timeout->get<long long>();
    if (seconds < 1 || seconds > std::numeric_limits<int>::max()) {
      throw std::runtime_error("timeout_seconds must be positive");
    }
    out.timeout_seconds = static_cast<int>(seconds);
  }
  out.read_only = external_read_only_capabilities(doc);
  out.dir = path.parent_path();
  return out;
}

Manifest parse_manifest(const fs::path& path) {
  std::ifstream in(path);
  if (!in) {
    throw std::runtime_error("cannot read manifest");
  }
  json doc;
  try {
    in >> doc;
  } catch (const std::exception& e) {
    throw std::runtime_error("invalid JSON: " + std::string(e.what()));
  }
  if (!doc.is_object()) {
    throw std::runtime_error("manifest must be an object");
  }
  Manifest out;
  out.name = string_field(doc, "name");
  if (out.name.empty()) {
    throw std::runtime_error("missing name");
  }
  auto command = doc.find("command");
  if (command == doc.end() || !command->is_array()) {
    throw std::runtime_error("command must be an array");
  }
  for (const auto& item : *command) {
    if (!item.is_string() || item.get<std::string>().empty()) {
      throw std::runtime_error("command entries must be strings");
    }
    out.command.push_back(item.get<std::string>());
  }
  if (out.command.empty()) {
    throw std::runtime_error("command must not be empty");
  }
  if (auto timeout = doc.find("response_timeout_seconds"); timeout != doc.end()) {
    if (timeout->is_null()) {
      out.timeout_ms = -1;
    } else if (timeout->is_number_integer() && timeout->get<int>() > 0) {
      out.timeout_ms = timeout->get<int>() * 1000;
    } else {
      throw std::runtime_error("response_timeout_seconds must be a positive integer or null");
    }
  }
  return out;
}

class Process {
public:
  std::string name;
  std::set<std::string> events;
  int timeout_ms = kDefaultTimeoutMs;
  std::mutex mutex;

  Process(const Manifest& manifest, const fs::path& dir, const fs::path& workspace)
      : name(manifest.name), timeout_ms(manifest.timeout_ms) {
    int input_pipe[2];
    int output_pipe[2];
    if (pipe(input_pipe) != 0 || pipe(output_pipe) != 0) {
      throw std::runtime_error(std::strerror(errno));
    }
    pid_ = fork();
    if (pid_ < 0) {
      throw std::runtime_error(std::strerror(errno));
    }
    if (pid_ == 0) {
      setpgid(0, 0);
      dup2(input_pipe[0], STDIN_FILENO);
      dup2(output_pipe[1], STDOUT_FILENO);
      int null_fd = open("/dev/null", O_WRONLY);
      if (null_fd >= 0) {
        dup2(null_fd, STDERR_FILENO);
      }
      close(input_pipe[0]);
      close(input_pipe[1]);
      close(output_pipe[0]);
      close(output_pipe[1]);
      if (null_fd >= 0) {
        close(null_fd);
      }
      if (chdir(workspace.c_str()) != 0) {
        _exit(127);
      }
      std::vector<std::string> command = manifest.command;
      if (!fs::path(command[0]).is_absolute() && (command[0].find('/') != std::string::npos ||
                                                  command[0].find('\\') != std::string::npos)) {
        command[0] = (dir / command[0]).lexically_normal().string();
      }
      std::vector<char*> argv;
      for (auto& value : command) {
        argv.push_back(value.data());
      }
      argv.push_back(nullptr);
      execvp(argv[0], argv.data());
      _exit(127);
    }
    close(input_pipe[0]);
    close(output_pipe[1]);
    input_ = input_pipe[1];
    output_ = output_pipe[0];
    fcntl(output_, F_SETFL, O_NONBLOCK);
  }

  ~Process() { stop(); }

  void send(const json& message) {
    std::lock_guard lock(send_mutex_);
    auto line = message.dump() + "\n";
    size_t offset = 0;
    while (offset < line.size()) {
      auto n = write(input_, line.data() + offset, line.size() - offset);
      if (n < 0) {
        if (errno == EINTR) {
          continue;
        }
        throw std::runtime_error("extension closed stdin");
      }
      offset += static_cast<size_t>(n);
    }
  }

  void start_reader(std::function<void(const std::string&)> callback) {
    on_line_ = std::move(callback);
    reader_stopping_.store(false);
    {
      std::lock_guard lock(state_mutex_);
      reader_exited_ = false;
      reader_error_.clear();
    }
    reader_ = std::thread([this] { read_loop(); });
  }

  void wait_for_response(const std::string& id, std::atomic<bool>* cancel, json& response) {
    std::unique_lock lock(state_mutex_);
    while (true) {
      auto it = responses_.find(id);
      if (it != responses_.end()) {
        response = std::move(it->second);
        responses_.erase(it);
        return;
      }
      if ((cancel != nullptr) && cancel->load()) {
        throw std::runtime_error("extension request cancelled");
      }
      if (reader_exited_) {
        throw std::runtime_error(reader_error_.empty() ? "extension exited before responding"
                                                       : reader_error_);
      }
      if (timeout_ms >= 0) {
        const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
                                 std::chrono::steady_clock::now() - last_activity_)
                                 .count();
        if (elapsed >= timeout_ms) {
          throw std::runtime_error("extension response timed out");
        }
      }
      state_cv_.wait_for(lock, std::chrono::milliseconds(50));
    }
  }

  void store_response(const std::string& id, json response) {
    std::lock_guard lock(state_mutex_);
    responses_[id] = std::move(response);
    last_activity_ = std::chrono::steady_clock::now();
    state_cv_.notify_all();
  }

  void mark_activity() {
    std::lock_guard lock(state_mutex_);
    last_activity_ = std::chrono::steady_clock::now();
  }

  std::string receive(int timeout, std::atomic<bool>* cancel) {
    auto started = std::chrono::steady_clock::now();
    while (true) {
      auto newline = buffer_.find('\n');
      if (newline != std::string::npos) {
        auto line = buffer_.substr(0, newline);
        buffer_.erase(0, newline + 1);
        if (!line.empty() && line.back() == '\r') {
          line.pop_back();
        }
        return line;
      }
      if ((cancel != nullptr) && cancel->load()) {
        throw std::runtime_error("extension request cancelled");
      }
      int wait_ms = 50;
      if (timeout >= 0) {
        auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
                           std::chrono::steady_clock::now() - started)
                           .count();
        if (elapsed >= timeout) {
          throw std::runtime_error("extension response timed out");
        }
        wait_ms = std::min(wait_ms, timeout - static_cast<int>(elapsed));
      }
      pollfd pfd{output_, POLLIN | POLLHUP, 0};
      int ready;
      do {
        ready = poll(&pfd, 1, wait_ms);
      } while (ready < 0 && errno == EINTR);
      if (ready < 0) {
        throw std::runtime_error(std::strerror(errno));
      }
      if (ready == 0) {
        continue;
      }
      char chunk[4096];
      auto count = read(output_, chunk, sizeof(chunk));
      if (count > 0) {
        buffer_.append(chunk, static_cast<size_t>(count));
        if (buffer_.size() > kMaxLineBytes) {
          throw std::runtime_error("extension response is too large");
        }
        continue;
      }
      if (count == 0 || ((pfd.revents & (POLLHUP | POLLERR)) != 0)) {
        throw std::runtime_error("extension exited before responding");
      }
      if (errno != EAGAIN && errno != EWOULDBLOCK) {
        throw std::runtime_error(std::strerror(errno));
      }
    }
  }

  void stop() {
    if (pid_ <= 0) {
      return;
    }
    try {
      send(json{{"type", "shutdown"}});
    } catch (...) {
    }
    reader_stopping_.store(true);
    close(input_);
    input_ = -1;
    if (reader_.joinable()) {
      reader_.join();
    }
    for (int i = 0; i < 10; ++i) {
      int status = 0;
      if (waitpid(pid_, &status, WNOHANG) == pid_) {
        close(output_);
        output_ = -1;
        pid_ = -1;
        return;
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }
    kill(-pid_, SIGKILL);
    kill(pid_, SIGKILL);
    waitpid(pid_, nullptr, 0);
    close(output_);
    output_ = -1;
    pid_ = -1;
  }

private:
  void read_loop() {
    std::string error;
    while (!reader_stopping_.load()) {
      bool had_line = false;
      while (true) {
        const auto newline = buffer_.find('\n');
        if (newline == std::string::npos) {
          break;
        }
        auto line = buffer_.substr(0, newline);
        buffer_.erase(0, newline + 1);
        if (!line.empty() && line.back() == '\r') {
          line.pop_back();
        }
        mark_activity();
        if (on_line_) {
          try {
            on_line_(line);
          } catch (const std::exception& e) {
            error = e.what();
            reader_stopping_.store(true);
            break;
          }
        }
        had_line = true;
      }
      if (reader_stopping_.load()) {
        break;
      }
      if (had_line) {
        continue;
      }
      pollfd pfd{output_, POLLIN | POLLHUP, 0};
      int ready;
      do {
        ready = poll(&pfd, 1, 50);
      } while (ready < 0 && errno == EINTR);
      if (ready <= 0) {
        continue;
      }
      char chunk[4096];
      const auto count = read(output_, chunk, sizeof(chunk));
      if (count == 0) {
        error = "extension exited before responding";
        break;
      }
      if (count < 0) {
        if (errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK) {
          continue;
        }
        error = std::strerror(errno);
        break;
      }
      buffer_.append(chunk, static_cast<size_t>(count));
      if (buffer_.size() > kMaxLineBytes) {
        error = "extension response is too large";
        break;
      }
    }
    {
      std::lock_guard lock(state_mutex_);
      reader_error_ = std::move(error);
      reader_exited_ = true;
    }
    state_cv_.notify_all();
  }

  pid_t pid_ = -1;
  int input_ = -1;
  int output_ = -1;
  std::string buffer_;
  std::mutex send_mutex_;
  std::mutex state_mutex_;
  std::condition_variable state_cv_;
  std::map<std::string, json> responses_;
  std::chrono::steady_clock::time_point last_activity_ = std::chrono::steady_clock::now();
  std::thread reader_;
  std::function<void(const std::string&)> on_line_;
  std::atomic<bool> reader_stopping_{false};
  bool reader_exited_ = false;
  std::string reader_error_;
};

std::string read_external_output(const fs::path& path) {
  std::ifstream in(path, std::ios::binary);
  if (!in) {
    return {};
  }
  std::ostringstream out;
  out << in.rdbuf();
  auto value = out.str();
  if (value.size() > kMaxExternalOutputBytes) {
    value.resize(kMaxExternalOutputBytes);
    value += "\n[truncated]";
  }
  return value;
}

fs::path external_executable(const ExternalTool& tool) {
  const fs::path command(tool.command.front());
  return command.is_absolute() ? command : (tool.dir / command).lexically_normal();
}

std::string external_failure(std::string message, const std::string& stdout_text,
                             const std::string& stderr_text) {
  if (stdout_text.empty() && !stderr_text.empty()) {
    message += "\n\nstderr:\n" + stderr_text;
  } else if (!stdout_text.empty()) {
    message += "\n\n" + stdout_text;
  }
  return "tool error: " + message;
}

std::string run_external_tool(const ExternalTool& tool, const json& input,
                              const fs::path& workspace, std::atomic<bool>* cancel) {
  const auto executable = external_executable(tool);
  std::error_code ec;
  if (!fs::is_regular_file(executable, ec)) {
    return "tool error: extension executable not found: " + executable.string();
  }

  const auto stamp = std::to_string(getpid()) + "-" +
                     std::to_string(std::chrono::steady_clock::now().time_since_epoch().count());
  const auto temp = fs::temp_directory_path();
  const auto input_path = temp / ("niminal-tool-" + stamp + ".in");
  const auto stdout_path = temp / ("niminal-tool-" + stamp + ".out");
  const auto stderr_path = temp / ("niminal-tool-" + stamp + ".err");
  auto cleanup = [&] {
    fs::remove(input_path, ec);
    fs::remove(stdout_path, ec);
    fs::remove(stderr_path, ec);
  };

  try {
    std::ofstream input_file(input_path, std::ios::binary | std::ios::trunc);
    if (!input_file) {
      throw std::runtime_error("cannot create tool input");
    }
    input_file << (input.is_null() ? json::object() : input).dump();
    input_file.close();
    std::ofstream(stdout_path, std::ios::binary | std::ios::trunc).close();
    std::ofstream(stderr_path, std::ios::binary | std::ios::trunc).close();

    const auto start = std::chrono::steady_clock::now();
    const auto pid = fork();
    if (pid < 0) {
      throw std::runtime_error(std::strerror(errno));
    }
    if (pid == 0) {
      setpgid(0, 0);
      const auto input_fd = open(input_path.c_str(), O_RDONLY);
      const auto stdout_fd = open(stdout_path.c_str(), O_WRONLY | O_TRUNC);
      const auto stderr_fd = open(stderr_path.c_str(), O_WRONLY | O_TRUNC);
      if (input_fd < 0 || stdout_fd < 0 || stderr_fd < 0) {
        _exit(126);
      }
      dup2(input_fd, STDIN_FILENO);
      dup2(stdout_fd, STDOUT_FILENO);
      dup2(stderr_fd, STDERR_FILENO);
      close(input_fd);
      close(stdout_fd);
      close(stderr_fd);
      if (chdir(workspace.c_str()) != 0) {
        _exit(127);
      }

      std::vector<std::string> command = tool.command;
      command.front() = executable.string();
      std::vector<char*> argv;
      for (auto& value : command) {
        argv.push_back(value.data());
      }
      argv.push_back(nullptr);
      execv(argv.front(), argv.data());
      _exit(127);
    }
    setpgid(pid, pid);

    int status = 0;
    bool finished = false;
    bool timed_out = false;
    bool interrupted = false;
    const auto deadline = start + std::chrono::seconds(tool.timeout_seconds);
    while (!finished) {
      const auto waited = waitpid(pid, &status, WNOHANG);
      if (waited == pid) {
        finished = true;
        break;
      }
      if (waited < 0 && errno != EINTR) {
        throw std::runtime_error(std::strerror(errno));
      }
      if ((cancel != nullptr) && cancel->load()) {
        interrupted = true;
      } else if (std::chrono::steady_clock::now() >= deadline) {
        timed_out = true;
      }
      if (interrupted || timed_out) {
        kill(-pid, SIGKILL);
        kill(pid, SIGKILL);
        waitpid(pid, &status, 0);
        finished = true;
        break;
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }

    const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
                             std::chrono::steady_clock::now() - start)
                             .count();
    const auto stdout_text = read_external_output(stdout_path);
    const auto stderr_text = read_external_output(stderr_path);
    cleanup();

    if (timed_out) {
      return external_failure("TIMEOUT after " + std::to_string(tool.timeout_seconds) + "s (" +
                                  std::to_string(elapsed) + "ms)",
                              stdout_text, stderr_text);
    }
    if (interrupted) {
      return external_failure("INTERRUPTED (" + std::to_string(elapsed) + "ms)", stdout_text,
                              stderr_text);
    }

    const int exit_code = WIFEXITED(status) ? WEXITSTATUS(status) : 1;
    bool valid_json = false;
    if (!stdout_text.empty()) {
      try {
        const auto parsed = json::parse(stdout_text);
        (void)parsed;
        valid_json = true;
      } catch (...) {
      }
    }
    if (!valid_json) {
      auto message = std::string("stdout was not valid JSON");
      if (!stdout_text.empty()) {
        message += "\n\n" + stdout_text;
      }
      if (stdout_text.empty() && !stderr_text.empty()) {
        message += "\n\nstderr:\n" + stderr_text;
      }
      return "tool error: " + message;
    }
    if (exit_code != 0) {
      return external_failure("exit_code: " + std::to_string(exit_code), stdout_text, stderr_text);
    }
    return stdout_text;
  } catch (...) {
    cleanup();
    throw;
  }
}

struct RegisteredTool {
  std::string name;
  std::string description;
  json schema;
  size_t extension = 0;
  bool read_only = false;
};

} // namespace

namespace {

std::string shell_quote(const std::string& value) {
  std::string out = "'";
  for (char c : value) {
    if (c == '\'') {
      out += "'\\''";
    } else {
      out += c;
    }
  }
  return out + "'";
}

} // namespace

std::string edit_text_externally(const std::string& text, const std::string& editor) {
  auto command = editor;
  if (command.empty()) {
    const char* visual = std::getenv("VISUAL");
    const char* env = (visual != nullptr) && ((*visual) != 0) ? visual : std::getenv("EDITOR");
    command = (env != nullptr) && ((*env) != 0) ? env : "nano";
  }
  const auto stamp = std::to_string(getpid()) + "-" +
                     std::to_string(std::chrono::steady_clock::now().time_since_epoch().count());
  const auto path = fs::temp_directory_path() / ("niminal-editor-" + stamp + ".md");
  try {
    {
      std::ofstream out(path, std::ios::binary | std::ios::trunc);
      if (!out) {
        throw std::runtime_error("cannot create editor buffer");
      }
      out << text;
    }
    const auto invocation = command + " " + shell_quote(path.string());
    const auto pid = fork();
    if (pid < 0) {
      throw std::runtime_error(std::strerror(errno));
    }
    if (pid == 0) {
      execl("/bin/sh", "sh", "-c", invocation.c_str(), nullptr);
      _exit(127);
    }
    int status = 0;
    if (waitpid(pid, &status, 0) < 0) {
      throw std::runtime_error(std::strerror(errno));
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
      throw std::runtime_error(command + " exited with code " +
                               std::to_string(WIFEXITED(status) ? WEXITSTATUS(status) : 1));
    }
    std::ifstream in(path, std::ios::binary);
    std::ostringstream result;
    result << in.rdbuf();
    const auto edited = result.str();
    std::error_code ec;
    fs::remove(path, ec);
    return edited;
  } catch (...) {
    std::error_code ec;
    fs::remove(path, ec);
    throw;
  }
}

struct ExtensionRuntime::Impl {
  std::vector<std::unique_ptr<Process>> processes;
  std::vector<RegisteredTool> tools;
  std::vector<ExternalTool> external_tools;
  mutable std::mutex actions_mutex;
  std::map<std::string, std::string> statuses;
  std::map<std::string, std::vector<std::string>> widgets;
  std::vector<ExtensionNotice> notices;
  std::vector<ExtensionUserMessage> user_messages;
  std::vector<ExtensionEntry> entries;
  std::atomic<bool> actions_pending{false};
  ExtensionUiCallbacks ui;
  std::function<void(const std::string&, const std::string&)> tool_update;
  std::function<json(const std::string&, const json&)> host_request;
  std::mutex callbacks_mutex;
  std::atomic<int> next_id{0};
  bool stopped = false;
};

const char* hook_event_name(HookEvent event) {
  switch (event) {
  case HookEvent::tool_call:
    return "tool_call";
  case HookEvent::tool_result:
    return "tool_result";
  case HookEvent::session_start:
    return "session_start";
  case HookEvent::session_end:
    return "session_end";
  case HookEvent::session_before_compact:
    return "session_before_compact";
  case HookEvent::session_compact:
    return "session_compact";
  case HookEvent::turn_start:
    return "turn_start";
  case HookEvent::turn_end:
    return "turn_end";
  case HookEvent::context:
    return "context";
  }
  return "";
}

ExtensionRuntime::ExtensionRuntime(Access, fs::path workspace, std::atomic<bool>* cancel)
    : impl_(std::make_unique<Impl>()), workspace_(std::move(workspace)), cancel_(cancel) {}

ExtensionRuntime::~ExtensionRuntime() {
  stop();
}

void ExtensionRuntime::set_ui_callbacks(ExtensionUiCallbacks callbacks) {
  std::lock_guard lock(impl_->callbacks_mutex);
  impl_->ui = std::move(callbacks);
}

std::string ExtensionRuntime::edit_text(const std::string& title, const std::string& text) {
  std::function<std::string(const std::string&, const std::string&)> editor;
  {
    std::lock_guard lock(impl_->callbacks_mutex);
    editor = impl_->ui.editor;
  }
  if (!editor) {
    throw std::runtime_error("interactive editor is unavailable");
  }
  return editor(title, text);
}

void ExtensionRuntime::set_host_request(
    std::function<json(const std::string&, const json&)> callback) {
  std::lock_guard lock(impl_->callbacks_mutex);
  impl_->host_request = std::move(callback);
}

void ExtensionRuntime::set_tool_update(
    std::function<void(const std::string&, const std::string&)> callback) {
  std::lock_guard lock(impl_->callbacks_mutex);
  impl_->tool_update = std::move(callback);
}

namespace {

void capture_actions(ExtensionRuntime::Impl& impl, const Process& process, const json& response) {
  std::lock_guard lock(impl.actions_mutex);
  if (auto status = response.find("status"); status != response.end() && status->is_object()) {
    auto key = string_field(*status, "key");
    auto text = string_field(*status, "text");
    if (!key.empty()) {
      key = process.name + ":" + key;
      if (text.empty()) {
        impl.statuses.erase(key);
      } else {
        impl.statuses[key] = std::move(text);
      }
    }
  }
  if (auto notice = response.find("notification");
      notice != response.end() && notice->is_object()) {
    auto message = string_field(*notice, "message");
    if (!message.empty()) {
      impl.notices.push_back({string_field(*notice, "level"), message});
    }
  }
  if (auto widget = response.find("widget"); widget != response.end() && widget->is_object()) {
    auto key = string_field(*widget, "key");
    auto lines = string_array(*widget, "lines");
    if (!key.empty()) {
      key = process.name + ":" + key;
      if (lines.empty()) {
        impl.widgets.erase(key);
      } else {
        impl.widgets[key] = std::move(lines);
      }
    }
  }
  if (auto entry = response.find("entry"); entry != response.end()) {
    impl.entries.push_back({process.name, *entry});
  }
  if (auto user = response.find("user_message"); user != response.end() && user->is_object()) {
    auto content = string_field(*user, "content");
    auto deliver_as = string_field(*user, "deliver_as");
    if (!content.empty() &&
        (deliver_as == "now" || deliver_as == "steer" || deliver_as == "follow_up")) {
      impl.user_messages.push_back({content, deliver_as});
    }
  }
  if (response.contains("status") || response.contains("notification") ||
      response.contains("widget") || response.contains("entry") ||
      response.contains("user_message")) {
    impl.actions_pending = true;
  }
}

void handle_incoming(ExtensionRuntime::Impl& impl, Process& process, const std::string& line) {
  json incoming;
  try {
    incoming = json::parse(line);
  } catch (...) {
    return;
  }
  const auto type = string_field(incoming, "type");
  if (type == "response") {
    const auto id = string_field(incoming, "id");
    process.store_response(id, std::move(incoming));
    return;
  }
  if (type == "ui_request") {
    const auto method = string_field(incoming, "method");
    ExtensionUiCallbacks callbacks;
    {
      std::lock_guard lock(impl.callbacks_mutex);
      callbacks = impl.ui;
    }
    json response{
        {"type", "ui_response"}, {"id", string_field(incoming, "id")}, {"cancelled", true}};
    try {
      if (method == "question") {
        const auto answer = callbacks.question
                                ? callbacks.question(string_field(incoming, "prompt"),
                                                     string_array(incoming, "options"))
                                : std::string();
        response["answer"] = answer;
        response["cancelled"] = answer.empty();
      } else if (method == "confirm") {
        const auto answer =
            callbacks.question ? callbacks.question(string_field(incoming, "prompt"), {"Yes", "No"})
                               : std::string();
        response["confirmed"] = answer == "Yes";
        response["cancelled"] = answer.empty();
      } else if (method == "input" || method == "password") {
        const auto answer = callbacks.input ? callbacks.input(string_field(incoming, "prompt"),
                                                              method == "password")
                                            : std::string();
        response["answer"] = answer;
        response["cancelled"] = answer.empty();
      }
    } catch (const std::exception& e) {
      response["error"] = e.what();
    }
    process.send(response);
    return;
  }
  if (type == "host_request") {
    std::function<json(const std::string&, const json&)> callback;
    {
      std::lock_guard lock(impl.callbacks_mutex);
      callback = impl.host_request;
    }
    json response{{"type", "host_response"}, {"id", string_field(incoming, "id")}};
    try {
      if (!callback) {
        response["cancelled"] = true;
        response["error"] = "host request is unavailable";
      } else {
        response["result"] = callback(string_field(incoming, "method"), incoming);
      }
    } catch (const Cancelled&) {
      response["cancelled"] = true;
    } catch (const std::exception& e) {
      response["error"] = e.what();
    }
    process.send(response);
    return;
  }
  if (type == "tool_update") {
    std::function<void(const std::string&, const std::string&)> callback;
    {
      std::lock_guard lock(impl.callbacks_mutex);
      callback = impl.tool_update;
    }
    if (callback) {
      try {
        callback(string_field(incoming, "id"), string_field(incoming, "content"));
      } catch (...) {
      }
    }
    return;
  }
  capture_actions(impl, process, incoming);
}

json request(ExtensionRuntime::Impl& impl, Process& process, const json& message,
             std::atomic<bool>* cancel) {
  const auto id = string_field(message, "id");
  process.mark_activity();
  process.send(message);
  try {
    json response;
    process.wait_for_response(id, cancel, response);
    capture_actions(impl, process, response);
    return response;
  } catch (...) {
    if ((cancel != nullptr) && cancel->load()) {
      try {
        process.send(json{{"type", "cancel"}, {"id", id}});
      } catch (...) {
      }
    }
    throw;
  }
}

bool read_only_capabilities(const json& tool) {
  auto it = tool.find("capabilities");
  if (it == tool.end()) {
    return false;
  }
  if (!it->is_array()) {
    throw std::runtime_error("capabilities must be an array");
  }
  bool any = false;
  bool read_only = true;
  for (const auto& item : *it) {
    if (!item.is_string()) {
      throw std::runtime_error("capabilities must be strings");
    }
    auto value = niminal::lower_copy(item.get<std::string>());
    if (value != "read" && value != "write" && value != "shell" && value != "network" &&
        value != "user") {
      throw std::runtime_error("unknown capability: " + value);
    }
    any = true;
    read_only = read_only && value == "read";
  }
  return any && read_only;
}

} // namespace

std::shared_ptr<ExtensionRuntime> ExtensionRuntime::start(const fs::path& workspace,
                                                          const std::string& session_id,
                                                          std::atomic<bool>* cancel) {
  auto runtime =
      std::make_shared<ExtensionRuntime>(Access{}, canonical_workspace(workspace), cancel);
  std::signal(SIGPIPE, SIG_IGN);
  for (const auto& dir : extension_dirs(runtime->workspace_)) {
    Manifest manifest;
    try {
      manifest = parse_manifest(dir / "extension.json");
    } catch (const std::exception& e) {
      runtime->warnings_.push_back("skipping " + dir.string() + ": " + e.what());
      continue;
    }
    std::unique_ptr<Process> process;
    const auto command_count = runtime->commands_.size();
    const auto tool_count = runtime->impl_->tools.size();
    try {
      process = std::make_unique<Process>(manifest, dir, runtime->workspace_);
      process->send(json{{"type", "initialize"},
                         {"version", 1},
                         {"workspace", runtime->workspace_.string()},
                         {"session_id", session_id}});
      auto registration = json::parse(process->receive(manifest.timeout_ms, cancel));
      if (string_field(registration, "type") != "register") {
        throw std::runtime_error("expected register response");
      }
      auto commands = registration.find("commands");
      if (commands == registration.end() || !commands->is_array()) {
        throw std::runtime_error("register commands must be an array");
      }
      auto events = registration.find("events");
      if (events != registration.end() && !events->is_array()) {
        throw std::runtime_error("register events must be an array");
      }
      if (events != registration.end()) {
        for (const auto& event : *events) {
          if (!event.is_string()) {
            throw std::runtime_error("register events must be strings");
          }
          process->events.insert(event.get<std::string>());
        }
      }
      size_t index = runtime->impl_->processes.size();
      for (const auto& command : *commands) {
        auto name = string_field(command, "name");
        if (!name.empty()) {
          runtime->commands_.push_back({name, string_field(command, "description"), index});
        }
      }
      auto tools = registration.find("tools");
      if (tools != registration.end() && !tools->is_null() && !tools->is_array()) {
        throw std::runtime_error("register tools must be an array");
      }
      if (tools != registration.end() && tools->is_array()) {
        for (const auto& tool : *tools) {
          auto name = string_field(tool, "name");
          auto description = string_field(tool, "description");
          auto schema = tool.find("input_schema");
          if (name.empty() || description.empty() || schema == tool.end() || !schema->is_object()) {
            throw std::runtime_error(
                "registered tools require name, description, and input_schema");
          }
          runtime->impl_->tools.push_back(
              {name, description, *schema, index, read_only_capabilities(tool)});
        }
      }
      runtime->impl_->processes.push_back(std::move(process));
      auto* active = runtime->impl_->processes.back().get();
      const std::weak_ptr<ExtensionRuntime> weak_runtime = runtime;
      active->start_reader([weak_runtime, active](const std::string& line) {
        if (auto runtime = weak_runtime.lock()) {
          handle_incoming(*runtime->impl_, *active, line);
        }
      });
    } catch (const std::exception& e) {
      runtime->commands_.resize(command_count);
      runtime->impl_->tools.resize(tool_count);
      if (process) {
        process->stop();
      }
      runtime->warnings_.push_back("extension '" + manifest.name +
                                   "' failed to start: " + e.what());
    }
  }
  std::map<std::string, ExternalTool> external_tools;
  for (const auto& dir : external_tool_dirs(runtime->workspace_)) {
    try {
      auto tool = parse_external_manifest(dir / "tool.json");
      if (builtin_tool(tool.name)) {
        runtime->warnings_.push_back("skipping external tool '" + tool.name +
                                     "': name collides with a built-in tool");
        continue;
      }
      external_tools[niminal::lower_copy(tool.name)] = std::move(tool);
    } catch (const std::exception& e) {
      runtime->warnings_.push_back("skipping " + (dir / "tool.json").string() + ": " + e.what());
    }
  }
  for (auto& [_, tool] : external_tools) {
    runtime->impl_->external_tools.push_back(std::move(tool));
  }
  return runtime;
}

std::vector<niminal::Tool> ExtensionRuntime::tools() {
  std::map<std::string, RegisteredTool> chosen;
  for (const auto& tool : impl_->tools) {
    chosen[niminal::lower_copy(tool.name)] = tool;
  }
  std::vector<niminal::Tool> result;
  auto self = shared_from_this();
  for (const auto& [_, tool] : chosen) {
    if (builtin_tool(tool.name)) {
      warnings_.push_back("extension tool '" + tool.name + "' collides with a built-in tool");
      continue;
    }
    result.push_back(niminal::Tool{tool.name, tool.description, tool.schema,
                                   [self, name = tool.name](const json& input) {
                                     auto response = self->invoke(name, input.dump());
                                     auto content = response.find("content");
                                     if (content == response.end() || !content->is_array()) {
                                       throw std::runtime_error(
                                           "extension tool content must be an array");
                                     }
                                     std::vector<std::string> text;
                                     for (const auto& part : *content) {
                                       if (string_field(part, "type") != "text") {
                                         continue;
                                       }
                                       text.push_back(string_field(part, "text"));
                                     }
                                     std::ostringstream output;
                                     for (size_t i = 0; i < text.size(); ++i) {
                                       if (i) {
                                         output << '\n';
                                       }
                                       output << text[i];
                                     }
                                     auto value = output.str();
                                     if (response.value("is_error", false)) {
                                       return std::string("tool error: ") + value;
                                     }
                                     return value;
                                   },
                                   tool.read_only, true});
  }
  for (const auto& tool : impl_->external_tools) {
    if (chosen.contains(niminal::lower_copy(tool.name))) {
      warnings_.push_back("skipping external tool '" + tool.name + "': name is already registered");
      continue;
    }
    result.push_back(niminal::Tool{tool.name, tool.description, tool.schema,
                                   [self, tool](const json& input) {
                                     return run_external_tool(tool, input, self->workspace_,
                                                              self->cancel_);
                                   },
                                   tool.read_only, true});
  }
  return result;
}

json ExtensionRuntime::invoke(const std::string& name, const std::string& arguments,
                              const json& context) {
  for (const auto& command : commands_) {
    if (niminal::lower_copy(command.name) != niminal::lower_copy(name)) {
      continue;
    }
    json message{{"type", "command"},
                 {"id", std::to_string(++impl_->next_id)},
                 {"name", command.name},
                 {"arguments", arguments}};
    if (!context.is_null() && !context.empty()) {
      message["context"] = context;
    }
    return request(*impl_, *impl_->processes[command.extension], std::move(message), cancel_);
  }
  for (auto it = impl_->tools.rbegin(); it != impl_->tools.rend(); ++it) {
    const auto& tool = *it;
    if (niminal::lower_copy(tool.name) != niminal::lower_copy(name)) {
      continue;
    }
    json input = json::object();
    if (!arguments.empty()) {
      input = json::parse(arguments);
    }
    return request(*impl_, *impl_->processes[tool.extension],
                   json{{"type", "tool"},
                        {"id", std::to_string(++impl_->next_id)},
                        {"name", tool.name},
                        {"arguments", input}},
                   cancel_);
  }
  throw std::runtime_error("unknown extension command or tool: " + name);
}

HookOutcome ExtensionRuntime::dispatch(HookEvent event, const json& original) {
  HookOutcome outcome;
  json payload = original.is_null() ? json::object() : original;
  for (auto& process : impl_->processes) {
    if (!process->events.contains(hook_event_name(event))) {
      continue;
    }
    try {
      auto response = request(*impl_, *process,
                              json{{"type", "event"},
                                   {"id", std::to_string(++impl_->next_id)},
                                   {"event", hook_event_name(event)},
                                   {"payload", payload}},
                              cancel_);
      if ((event == HookEvent::tool_call || event == HookEvent::session_before_compact) &&
          response.contains("allow") && response["allow"].is_boolean() &&
          !response["allow"].get<bool>()) {
        outcome.allowed = false;
        auto reason = string_field(response, "reason");
        if (!reason.empty()) {
          if (!outcome.reason.empty()) {
            outcome.reason += "; ";
          }
          outcome.reason += reason;
        }
        continue;
      }
      if (event == HookEvent::tool_call) {
        auto arguments = response.find("arguments");
        if (arguments != response.end() && arguments->is_object()) {
          outcome.arguments = *arguments;
          outcome.has_arguments = true;
          payload["arguments"] = *arguments;
        }
      } else if (event == HookEvent::tool_result) {
        auto output = response.find("output");
        if (output != response.end() && output->is_string()) {
          outcome.output = output->get<std::string>();
          outcome.has_output = true;
          payload["output"] = *output;
        }
        auto is_error = response.find("is_error");
        if (is_error != response.end() && is_error->is_boolean()) {
          outcome.is_error = is_error->get<bool>();
          outcome.has_is_error = true;
          payload["is_error"] = *is_error;
        }
      } else if (event == HookEvent::context) {
        auto system = string_array(response, "system");
        outcome.system.insert(outcome.system.end(), system.begin(), system.end());
        auto messages = response.find("messages");
        if (messages != response.end() && messages->is_array()) {
          for (const auto& message : *messages) {
            outcome.messages.push_back(message);
          }
        }
      } else if (event == HookEvent::session_before_compact) {
        auto instruction = response.find("instruction");
        if (instruction != response.end() && instruction->is_string()) {
          outcome.instruction = instruction->get<std::string>();
        }
        auto compaction = response.find("compaction");
        if (compaction != response.end() && compaction->is_object() &&
            compaction->contains("summary") && (*compaction)["summary"].is_string() &&
            compaction->contains("first_kept_index") &&
            (*compaction)["first_kept_index"].is_number_integer()) {
          outcome.summary = (*compaction)["summary"].get<std::string>();
          outcome.first_kept_index = (*compaction)["first_kept_index"].get<int>();
          outcome.details = compaction->value("details", json());
          outcome.has_compaction = !outcome.summary.empty();
        }
      }
    } catch (const std::exception& e) {
      outcome.warnings.push_back("extension '" + process->name + "': " + e.what());
    }
  }
  if (!outcome.allowed && outcome.reason.empty()) {
    outcome.reason = "blocked by extension";
  }
  return outcome;
}

bool ExtensionRuntime::pump() {
  return impl_->actions_pending.exchange(false);
}

void ExtensionRuntime::stop() {
  if (!impl_ || impl_->stopped) {
    return;
  }
  impl_->stopped = true;
  for (auto& process : impl_->processes) {
    process->stop();
  }
  impl_->processes.clear();
  commands_.clear();
  impl_->tools.clear();
  impl_->external_tools.clear();
}

std::vector<ExtensionNotice> ExtensionRuntime::take_notices() {
  std::lock_guard lock(impl_->actions_mutex);
  auto out = std::move(impl_->notices);
  impl_->notices.clear();
  return out;
}

std::vector<ExtensionUserMessage> ExtensionRuntime::take_user_messages() {
  std::lock_guard lock(impl_->actions_mutex);
  auto out = std::move(impl_->user_messages);
  impl_->user_messages.clear();
  return out;
}

std::vector<ExtensionEntry> ExtensionRuntime::take_entries() {
  std::lock_guard lock(impl_->actions_mutex);
  auto out = std::move(impl_->entries);
  impl_->entries.clear();
  return out;
}

std::vector<std::string> ExtensionRuntime::status_texts() const {
  std::lock_guard lock(impl_->actions_mutex);
  std::vector<std::string> out;
  for (const auto& [_, text] : impl_->statuses) {
    out.push_back(text);
  }
  return out;
}

std::vector<std::string> ExtensionRuntime::widget_lines() const {
  std::lock_guard lock(impl_->actions_mutex);
  std::vector<std::string> out;
  for (const auto& [_, lines] : impl_->widgets) {
    out.insert(out.end(), lines.begin(), lines.end());
  }
  return out;
}

json session_hook_payload(const std::string& session_id, const fs::path& workspace) {
  return json{{"session_id", session_id}, {"workspace", workspace.string()}};
}

void bind_extensions(niminal::Agent& agent, const std::shared_ptr<ExtensionRuntime>& runtime,
                     const fs::path& workspace, const std::function<void(const std::string&)>& note,
                     Session* session, const Config& cfg) {
  if (runtime) {
    const std::weak_ptr<ExtensionRuntime> weak_runtime = runtime;
    runtime->set_host_request([&agent, session, weak_runtime, cfg](const std::string& method,
                                                                   const json& request) {
      if (method == "model.complete") {
        const auto prompt = request.value("prompt", std::string());
        if (prompt.empty()) {
          throw std::invalid_argument("prompt is required");
        }
        niminal::ChatRequest completion;
        agent.fill_chat(completion);
        completion.messages = json::array();
        const auto system = request.value("system_prompt", std::string());
        if (!system.empty()) {
          completion.messages.push_back(json{{"role", "system"}, {"content", system}});
        }
        completion.messages.push_back(json{{"role", "user"}, {"content", prompt}});
        completion.tools = json::array();
        completion.max_tokens = request.value("max_tokens", 0);
        completion.conversation_id = (session ? session->id : agent.conversation_id) + ":extension";
        completion.stream = false;
        completion.on_event = {};
        const auto text = niminal::complete_chat(completion);
        return json{{"text", text}, {"model", agent.model}, {"finish_reason", "stop"}};
      }
      if (method == "ui.editor") {
        auto runtime = weak_runtime.lock();
        if (!runtime) {
          throw std::runtime_error("extension host is unavailable");
        }
        return json{{"text", runtime->edit_text(request.value("title", std::string()),
                                                request.value("text", std::string()))}};
      }
      if (method == "session.info") {
        if (!session) {
          throw std::runtime_error("session is unavailable");
        }
        return json{{"id", session->id},
                    {"name", session->name},
                    {"path", session->path},
                    {"workspace", session->workspace},
                    {"event_count", session->events.size()}};
      }
      if (method == "session.name") {
        if (!session) {
          throw std::runtime_error("session is unavailable");
        }
        if (request.contains("name")) {
          session->add_name(request.value("name", std::string()));
        }
        return json{{"name", session->name}};
      }
      if (method == "context.usage") {
        if (!session) {
          throw std::runtime_error("session is unavailable");
        }
        const auto tokens = estimate_session_tokens(*session);
        const auto limit = cfg.context_window > 0 ? cfg.context_window : kDefaultContextWindow;
        return json{
            {"tokens", tokens}, {"limit", limit}, {"percent", std::min(100, tokens * 100 / limit)}};
      }
      throw std::runtime_error("unknown host request: " + method);
    });
  }
  auto report = [note](const HookOutcome& outcome) {
    if (!note) {
      return;
    }
    for (const auto& warning : outcome.warnings) {
      note(warning);
    }
  };
  agent.before_tool = [runtime, report](const niminal::ToolCall& call, json& args,
                                        std::string& reason) {
    if (!runtime) {
      return true;
    }
    auto outcome =
        runtime->dispatch(HookEvent::tool_call, json{{"tool", call.name}, {"arguments", args}});
    report(outcome);
    if (outcome.has_arguments) {
      args = std::move(outcome.arguments);
    }
    reason = outcome.reason;
    return outcome.allowed;
  };
  agent.after_tool = [runtime, report](const niminal::ToolCall& call, const json& args,
                                       std::string& output, bool& is_error) {
    if (!runtime) {
      return;
    }
    auto outcome = runtime->dispatch(
        HookEvent::tool_result,
        json{{"tool", call.name}, {"arguments", args}, {"output", output}, {"is_error", is_error}});
    report(outcome);
    if (outcome.has_output) {
      output = std::move(outcome.output);
    }
    if (outcome.has_is_error) {
      is_error = outcome.is_error;
    }
  };
  agent.augment_context = [runtime, report](json& messages) {
    if (!runtime) {
      return;
    }
    json system = json::array();
    json conversation = json::array();
    for (const auto& message : messages) {
      if (message.value("role", "") == "system") {
        auto content = message.find("content");
        if (content != message.end() && content->is_array()) {
          for (const auto& part : *content) {
            if (part.value("type", "") == "text") {
              system.push_back(part.value("text", ""));
            }
          }
        }
      } else {
        conversation.push_back(message);
      }
    }
    auto outcome =
        runtime->dispatch(HookEvent::context, json{{"system", system}, {"messages", conversation}});
    report(outcome);
    if (!outcome.system.empty()) {
      auto system_it = std::find_if(messages.begin(), messages.end(), [](const json& msg) {
        return msg.value("role", "") == "system";
      });
      if (system_it == messages.end()) {
        json parts = json::array();
        for (const auto& text : outcome.system) {
          parts.push_back(json{{"type", "text"}, {"text", text}});
        }
        messages.insert(messages.begin(), json{{"role", "system"}, {"content", parts}});
      } else {
        if (!(*system_it)["content"].is_array()) {
          (*system_it)["content"] = json::array();
        }
        for (const auto& text : outcome.system) {
          (*system_it)["content"].push_back(json{{"type", "text"}, {"text", text}});
        }
      }
    }
    for (const auto& message : outcome.messages) {
      messages.push_back(message);
    }
  };
  agent.turn_start = [runtime, workspace, &agent, report] {
    if (!runtime) {
      return;
    }
    auto outcome = runtime->dispatch(HookEvent::turn_start,
                                     session_hook_payload(agent.conversation_id, workspace));
    report(outcome);
  };
  agent.turn_end = [runtime, workspace, &agent, report](bool interrupted) {
    if (!runtime) {
      return;
    }
    auto payload = session_hook_payload(agent.conversation_id, workspace);
    if (interrupted) {
      payload["interrupted"] = true;
    }
    auto outcome = runtime->dispatch(HookEvent::turn_end, payload);
    report(outcome);
  };
}

void install_extension_tools(niminal::Agent& agent,
                             const std::shared_ptr<ExtensionRuntime>& runtime,
                             const std::vector<std::string>* allowed) {
  agent.tools.erase(std::remove_if(agent.tools.begin(), agent.tools.end(),
                                   [](const niminal::Tool& tool) { return tool.extension; }),
                    agent.tools.end());
  if (!runtime) {
    return;
  }
  auto tools = runtime->tools();
  if (allowed != nullptr) {
    tools.erase(std::remove_if(tools.begin(), tools.end(),
                               [&](const auto& tool) {
                                 auto name = niminal::lower_copy(tool.name);
                                 return std::find(allowed->begin(), allowed->end(), name) ==
                                        allowed->end();
                               }),
                tools.end());
  }
  agent.tools.insert(agent.tools.end(), std::make_move_iterator(tools.begin()),
                     std::make_move_iterator(tools.end()));
}

} // namespace niminal::app
