#include "extensions.hpp"

#include "provider.hpp"
#include <niminal/text.hpp>

#include "compaction.hpp"
#include "config.hpp"
#include "session.hpp"
#include "trust.hpp"

#include <niminal/chat.hpp>

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
#include <initializer_list>
#include <limits>
#include <map>
#include <mutex>
#include <optional>
#include <poll.h>
#include <set>
#include <sstream>
#include <stdexcept>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>
#include <utility>

extern char** environ;

namespace niminal::app {
namespace fs = std::filesystem;
using json = nlohmann::json;

namespace {

constexpr int kDefaultTimeoutMs = 30'000;
constexpr size_t kMaxLineBytes = 1'000'000;
constexpr int kDefaultExternalTimeoutSeconds = 30;
constexpr size_t kMaxExternalOutputBytes = 100'000;
// How long an extension gets to finish stopping what it started before its
// process group is killed. Extensions stop their own children (a subagent, a
// helper process), and the escalation they use has to fit inside this window:
// killing the extension first orphans the grandchildren it was stopping.
// spawn_agent gives a subagent 1.5s to exit before it SIGKILLs it.
constexpr auto kStopGrace = std::chrono::seconds(3);

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
  static const std::set<std::string> names = {"ask_user", "bash", "edit",  "glob", "grep",
                                              "ls",       "read", "skill", "write"};
  return names.contains(name);
}

std::vector<fs::path> scan_manifest_dirs(const std::vector<fs::path>& bases,
                                         const char* manifest_name) {
  std::vector<fs::path> result;
  std::error_code ec;
  for (const auto& base : bases) {
    if (!fs::is_directory(base, ec)) {
      continue;
    }
    std::vector<fs::path> found;
    for (const auto& entry : fs::directory_iterator(base, ec)) {
      if (entry.is_directory(ec) && fs::is_regular_file(entry.path() / manifest_name, ec)) {
        found.push_back(entry.path());
      }
    }
    std::sort(found.begin(), found.end());
    result.insert(result.end(), found.begin(), found.end());
  }
  return result;
}

std::vector<fs::path> resource_dirs(const fs::path& workspace, const char* global_name,
                                    std::initializer_list<const char*> local_names,
                                    const char* manifest_name) {
  std::vector<fs::path> bases;
  try {
    const auto global = config_path().parent_path();
    bases.push_back(global.parent_path() / ".agents" / global_name);
    bases.push_back(global / global_name);
  } catch (...) {
  }
  if (project_resources_trusted(workspace)) {
    for (const auto* name : local_names) {
      bases.push_back(workspace / name);
    }
  }
  return scan_manifest_dirs(bases, manifest_name);
}

std::vector<fs::path> extension_dirs(const fs::path& workspace) {
  return resource_dirs(workspace, "extensions", {".agents/extensions", ".niminal/extensions"},
                       "extension.json");
}

std::vector<fs::path> external_tool_dirs(const fs::path& workspace) {
  return resource_dirs(workspace, "tools", {".agent/tools", ".agents/tools", ".niminal/tools"},
                       "tool.json");
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

bool read_only_capabilities(const json& doc) {
  auto it = doc.find("capabilities");
  if (it == doc.end() || it->is_null()) {
    return false;
  }
  if (!it->is_array()) {
    throw std::runtime_error("capabilities must be an array");
  }
  bool any = false;
  bool writable = false;
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
    if (value != "read" && value != "user") {
      writable = true;
    }
  }
  return any && !writable;
}

json read_manifest_json(const fs::path& path) {
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
  return doc;
}

std::vector<std::string> read_command(const json& doc, bool require_nonempty_array,
                                      const char* array_error, const char* entry_error,
                                      const char* empty_error = nullptr) {
  const auto command = doc.find("command");
  if (command == doc.end() || !command->is_array() ||
      (require_nonempty_array && command->empty())) {
    throw std::runtime_error(array_error);
  }
  std::vector<std::string> result;
  for (const auto& item : *command) {
    if (!item.is_string() || (require_nonempty_array && item.get<std::string>().empty())) {
      throw std::runtime_error(entry_error);
    }
    result.push_back(item.get<std::string>());
  }
  if (result.empty() && empty_error != nullptr) {
    throw std::runtime_error(empty_error);
  }
  return result;
}

ExternalTool parse_external_manifest(const fs::path& path) {
  const auto doc = read_manifest_json(path);
  ExternalTool out;
  out.name = string_field(doc, "name");
  out.description = string_field(doc, "description");
  if (out.name.empty()) {
    throw std::runtime_error("missing name");
  }
  if (out.description.empty()) {
    throw std::runtime_error("missing description");
  }

  out.command = read_command(doc, true, "command must be a nonempty array",
                             "command entries must be nonempty strings");

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
  out.read_only = read_only_capabilities(doc);
  out.dir = path.parent_path();
  return out;
}

Manifest parse_manifest(const fs::path& path) {
  const auto doc = read_manifest_json(path);
  Manifest out;
  out.name = string_field(doc, "name");
  if (out.name.empty()) {
    throw std::runtime_error("missing name");
  }
  out.command = read_command(doc, false, "command must be an array",
                             "command entries must be strings", "command must not be empty");
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

// Composes the inherited environment with the shell env block and execs the
// command, searching PATH when the program is not path-qualified. Runs in the
// freshly forked child only.
void exec_with_env(const std::string& file, std::vector<std::string> command,
                   const ShellEnv& extra) {
  command.front() = file;
  std::vector<char*> argv;
  for (auto& value : command) {
    argv.push_back(value.data());
  }
  argv.push_back(nullptr);
  auto entries = child_environment(extra, environ);
  std::vector<char*> envp;
  envp.reserve(entries.size() + 1);
  for (auto& entry : entries) {
    envp.push_back(entry.data());
  }
  envp.push_back(nullptr);
  if (file.find('/') != std::string::npos) {
    execve(file.c_str(), argv.data(), envp.data());
    _exit(127);
  }
  std::string path;
  if (const char* env_path = std::getenv("PATH")) {
    path = env_path;
  }
  size_t start = 0;
  while (true) {
    const auto end = path.find(':', start);
    const auto dir = path.substr(start, end == std::string::npos ? std::string::npos : end - start);
    if (!dir.empty()) {
      execve((dir + "/" + file).c_str(), argv.data(), envp.data());
    }
    if (end == std::string::npos) {
      break;
    }
    start = end + 1;
  }
  _exit(127);
}

class Process {
public:
  std::string name;
  std::set<std::string> events;
  int timeout_ms = kDefaultTimeoutMs;
  std::mutex mutex;

  Process(const Manifest& manifest, const fs::path& dir, const fs::path& workspace,
          const ShellEnv& env)
      : name(manifest.name), timeout_ms(manifest.timeout_ms) {
    int input_pipe[2];
    int output_pipe[2];
    if (pipe(input_pipe) != 0 || pipe(output_pipe) != 0) {
      throw std::runtime_error(std::strerror(errno));
    }
    for (int fd : {input_pipe[0], input_pipe[1], output_pipe[0], output_pipe[1]}) {
      close_on_exec(fd);
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
      exec_with_env(command[0], command, env);
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

  void wait_for_response(const std::string& id, niminal::Cancellation* cancel, json& response) {
    std::unique_lock lock(state_mutex_);
    while (true) {
      auto it = responses_.find(id);
      if (it != responses_.end()) {
        response = std::move(it->second);
        responses_.erase(it);
        return;
      }
      if (reader_exited_) {
        throw std::runtime_error(reader_error_.empty()
                                     ? "extension '" + name + "' exited before responding"
                                     : reader_error_);
      }
      if ((cancel != nullptr) && cancel->requested()) {
        throw Cancelled();
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

  std::string receive(int timeout, niminal::Cancellation* cancel) {
    auto started = std::chrono::steady_clock::now();
    while (true) {
      if (auto line = take_line()) {
        return std::move(*line);
      }
      if ((cancel != nullptr) && cancel->requested()) {
        throw Cancelled();
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
      std::string error;
      if (!read_chunk(wait_ms, error, true) && !error.empty()) {
        throw std::runtime_error(error);
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
    // An extension may still be stopping something it started (a subagent, a
    // child process), so give it a moment before killing it.
    const auto deadline = std::chrono::steady_clock::now() + kStopGrace;
    while (std::chrono::steady_clock::now() < deadline) {
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
  bool read_chunk(int wait_ms, std::string& error, bool report_poll_error) {
    pollfd pfd{output_, POLLIN | POLLHUP, 0};
    int ready;
    do {
      ready = poll(&pfd, 1, wait_ms);
    } while (ready < 0 && errno == EINTR);
    if (ready < 0) {
      if (!report_poll_error) {
        return false;
      }
      error = std::strerror(errno);
      return false;
    }
    if (ready == 0) {
      return false;
    }
    char chunk[4096];
    const auto count = read(output_, chunk, sizeof(chunk));
    if (count > 0) {
      buffer_.append(chunk, static_cast<size_t>(count));
      if (buffer_.size() > kMaxLineBytes) {
        error = "extension response is too large";
        return false;
      }
      return true;
    }
    if (count == 0 || ((pfd.revents & (POLLHUP | POLLERR)) != 0)) {
      error = "extension '" + name + "' exited before responding";
      return false;
    }
    if (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) {
      return false;
    }
    error = std::strerror(errno);
    return false;
  }

  std::optional<std::string> take_line() {
    const auto newline = buffer_.find('\n');
    if (newline == std::string::npos) {
      return std::nullopt;
    }
    auto line = buffer_.substr(0, newline);
    buffer_.erase(0, newline + 1);
    if (!line.empty() && line.back() == '\r') {
      line.pop_back();
    }
    return line;
  }

  void read_loop() {
    std::string error;
    while (!reader_stopping_.load()) {
      bool had_line = false;
      while (true) {
        auto line = take_line();
        if (!line) {
          break;
        }
        mark_activity();
        if (on_line_) {
          try {
            on_line_(*line);
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
      std::string read_error;
      if (read_chunk(50, read_error, false) || read_error.empty()) {
        continue;
      }
      if (!read_error.empty()) {
        error = std::move(read_error);
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
                              const fs::path& workspace, niminal::Cancellation* cancel,
                              const ShellEnv& env) {
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
      exec_with_env(executable.string(), command, env);
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
      if ((cancel != nullptr) && cancel->requested()) {
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
      return external_failure("stdout was not valid JSON", stdout_text, stderr_text);
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
  std::vector<std::string> provider_names;
  std::vector<RegisteredTool> tools;
  std::vector<ExternalTool> external_tools;
  ShellEnvFn shell_env;
  mutable std::mutex actions_mutex;
  std::map<std::string, ExtensionStatus> statuses;
  std::map<std::string, ExtensionWidget> widgets;
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

template <typename T> std::vector<T> take_values(std::mutex& mutex, std::vector<T>& values) {
  std::lock_guard lock(mutex);
  auto out = std::move(values);
  values.clear();
  return out;
}

template <typename Map> auto copy_values(const Map& values) {
  std::vector<typename Map::mapped_type> out;
  out.reserve(values.size());
  for (const auto& [_, value] : values) {
    out.push_back(value);
  }
  return out;
}

const char* hook_event_name(HookEvent event) {
  static constexpr const char* names[] = {"tool_call",
                                          "tool_result",
                                          "session_start",
                                          "session_end",
                                          "session_before_compact",
                                          "session_compact",
                                          "turn_start",
                                          "turn_end",
                                          "context",
                                          "before_agent_start",
                                          "input",
                                          "session_shutdown",
                                          "session_before_switch",
                                          "before_provider_headers",
                                          "before_provider_request",
                                          "after_provider_response",
                                          "agent_settled",
                                          "message_end",
                                          "session_compact_failed"};
  const auto index = static_cast<size_t>(event);
  return index < sizeof(names) / sizeof(*names) ? names[index] : "";
}

ExtensionRuntime::ExtensionRuntime(Access, fs::path workspace, niminal::Cancellation* cancel)
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
    ExtensionStatus item{process.name, key, {}};
    bool valid = true;
    auto segments = status->find("segments");
    if (segments == status->end() || !segments->is_array()) {
      valid = false;
    } else {
      static const std::set<std::string> styles = {"normal",  "muted", "accent",  "success",
                                                   "warning", "error", "emphasis"};
      for (const auto& segment : *segments) {
        if (!segment.is_object()) {
          valid = false;
          break;
        }
        const auto style = string_field(segment, "style");
        if (!segment.contains("text") || !segment["text"].is_string() ||
            (!style.empty() && !styles.contains(style))) {
          valid = false;
          break;
        }
        item.segments.push_back({segment["text"].get<std::string>(), style});
      }
    }
    if (valid && !key.empty()) {
      key = process.name + ":" + key;
      if (item.segments.empty()) {
        impl.statuses.erase(key);
      } else {
        impl.statuses[key] = std::move(item);
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
    ExtensionWidget item;
    item.extension = process.name;
    item.key = key;
    item.position = string_field(*widget, "position");
    if (item.position.empty()) {
      item.position = "above_composer";
    }
    item.title = string_field(*widget, "title");
    bool valid = item.position == "above_composer";
    if (auto content = widget->find("content"); content != widget->end() && content->is_array()) {
      item.content = *content;
    } else {
      valid = false;
    }
    if (auto actions = widget->find("actions"); actions != widget->end()) {
      if (!actions->is_array() || actions->size() > 8) {
        valid = false;
      } else {
        for (const auto& action : *actions) {
          if (!action.is_object()) {
            valid = false;
            break;
          }
          const auto id = string_field(action, "id");
          const auto label = string_field(action, "label");
          if (id.empty() || label.empty() ||
              std::any_of(item.actions.begin(), item.actions.end(),
                          [&](const auto& existing) { return existing.id == id; })) {
            valid = false;
            break;
          }
          item.actions.push_back({id, label});
        }
      }
    }
    if (item.content.size() > 24) {
      valid = false;
    }
    for (const auto& element : item.content) {
      if (!element.is_object()) {
        valid = false;
        break;
      }
      const auto type = string_field(element, "type");
      if (type == "text") {
        if (!element.contains("text") || !element["text"].is_string()) {
          valid = false;
          break;
        }
      } else if (type == "list") {
        if (!element.contains("items") || !element["items"].is_array() ||
            element["items"].size() > 32) {
          valid = false;
          break;
        }
        for (const auto& entry : element["items"]) {
          if (!entry.is_object()) {
            valid = false;
            break;
          }
          const auto state = string_field(entry, "state");
          if (!entry.contains("text") || !entry["text"].is_string() ||
              (!state.empty() && state != "pending" && state != "active" && state != "done")) {
            valid = false;
            break;
          }
        }
      } else if (type == "progress") {
        if (!element.contains("value") || !element["value"].is_number() ||
            !element.contains("max") || !element["max"].is_number() ||
            element["max"].get<double>() <= 0 ||
            (element.contains("label") && !element["label"].is_string())) {
          valid = false;
          break;
        }
      } else {
        valid = false;
        break;
      }
    }
    if (valid && !key.empty()) {
      key = process.name + ":" + key;
      if (item.content.empty() && item.title.empty() && item.actions.empty()) {
        impl.widgets.erase(key);
      } else {
        impl.widgets[key] = std::move(item);
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
             niminal::Cancellation* cancel) {
  const auto id = string_field(message, "id");
  process.mark_activity();
  process.send(message);
  try {
    json response;
    process.wait_for_response(id, cancel, response);
    capture_actions(impl, process, response);
    return response;
  } catch (...) {
    if ((cancel != nullptr) && cancel->requested()) {
      try {
        process.send(json{{"type", "cancel"}, {"id", id}});
      } catch (...) {
      }
    }
    throw;
  }
}

} // namespace

std::shared_ptr<ExtensionRuntime> ExtensionRuntime::start(const fs::path& workspace,
                                                          const std::string& session_id,
                                                          niminal::Cancellation* cancel,
                                                          const ShellEnvFn* shell_env) {
  auto runtime =
      std::make_shared<ExtensionRuntime>(Access{}, canonical_workspace(workspace), cancel);
  if (shell_env != nullptr) {
    runtime->impl_->shell_env = *shell_env;
  }
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
      process = std::make_unique<Process>(manifest, dir, runtime->workspace_,
                                          runtime->impl_->shell_env ? runtime->impl_->shell_env()
                                                                    : ShellEnv{});
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
      std::vector<CustomProvider> providers;
      if (auto registered = registration.find("providers"); registered != registration.end()) {
        if (!registered->is_array()) {
          throw std::runtime_error("register providers must be an array");
        }
        for (const auto& provider : *registered) {
          if (!provider.is_object() || string_field(provider, "api") != "openai-chat-completions") {
            throw std::runtime_error("extension providers must use openai-chat-completions");
          }
          CustomProvider spec;
          spec.name = string_field(provider, "name");
          spec.endpoint = string_field(provider, "api_url");
          spec.default_model = string_field(provider, "default_model");
          spec.models = string_array(provider, "models");
          for (const auto& key : string_array(provider, "api_key_env")) {
            spec.env_keys.push_back(key);
          }
          spec.session_routing = provider.value("session_routing", false);
          spec.stream_usage = provider.value("stream_usage", false);
          spec.apply_cache = provider.value("apply_cache", false);
          spec.prompt_cache_key = provider.value("prompt_cache_key", false);
          spec.requires_api_key = provider.value("requires_api_key", true);
          if (spec.name.empty() || spec.endpoint.empty() || spec.default_model.empty() ||
              (spec.requires_api_key && spec.env_keys.empty())) {
            throw std::runtime_error(
                "extension providers require name, api_url, default_model, and api_key_env "
                "when authentication is required");
          }
          providers.push_back(std::move(spec));
        }
      }
      runtime->impl_->processes.push_back(std::move(process));
      auto* active = runtime->impl_->processes.back().get();
      const std::weak_ptr<ExtensionRuntime> weak_runtime = runtime;
      active->start_reader([weak_runtime, active](const std::string& line) {
        if (auto locked = weak_runtime.lock()) {
          handle_incoming(*locked->impl_, *active, line);
        }
      });
      for (auto& provider : providers) {
        const auto name = provider.name;
        if (!niminal::app::register_provider(std::move(provider))) {
          runtime->warnings_.push_back("extension '" + manifest.name +
                                       "' could not register provider '" + name + "'");
        } else {
          runtime->impl_->provider_names.push_back(name);
        }
      }
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
                                     std::string value;
                                     bool first = true;
                                     for (const auto& part : *content) {
                                       if (string_field(part, "type") != "text") {
                                         continue;
                                       }
                                       if (!first) {
                                         value += '\n';
                                       }
                                       value += string_field(part, "text");
                                       first = false;
                                     }
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
    result.push_back(niminal::Tool{
        tool.name, tool.description, tool.schema,
        [self, tool](const json& input) {
          ShellEnv env = self->impl_->shell_env ? self->impl_->shell_env() : ShellEnv{};
          return run_external_tool(tool, input, self->workspace_, self->cancel_, env);
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
  auto copy_string = [&](const json& response, const char* key, std::string& target,
                         bool& present) {
    if (auto value = response.find(key); value != response.end() && value->is_string()) {
      target = value->get<std::string>();
      present = true;
      payload[key] = *value;
    }
  };
  auto copy_bool = [&](const json& response, const char* key, bool& target, bool& present) {
    if (auto value = response.find(key); value != response.end() && value->is_boolean()) {
      target = value->get<bool>();
      present = true;
      payload[key] = *value;
    }
  };
  auto copy_object = [&](const json& response, const char* key, json& target, bool& present) {
    if (auto value = response.find(key); value != response.end() && value->is_object()) {
      target = *value;
      present = true;
      payload[key] = *value;
    }
  };
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
      if ((event == HookEvent::tool_call || event == HookEvent::session_before_compact ||
           event == HookEvent::session_before_switch || event == HookEvent::input) &&
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
        copy_object(response, "arguments", outcome.arguments, outcome.has_arguments);
      } else if (event == HookEvent::tool_result) {
        copy_string(response, "output", outcome.output, outcome.has_output);
        copy_bool(response, "is_error", outcome.is_error, outcome.has_is_error);
      } else if (event == HookEvent::context) {
        auto system = string_array(response, "system");
        outcome.system.insert(outcome.system.end(), system.begin(), system.end());
        auto messages = response.find("messages");
        if (messages != response.end() && messages->is_array()) {
          outcome.messages.insert(outcome.messages.end(), messages->begin(), messages->end());
        }
      } else if (event == HookEvent::input || event == HookEvent::message_end) {
        copy_string(response, "text", outcome.text, outcome.has_text);
      } else if (event == HookEvent::before_agent_start) {
        copy_string(response, "system_prompt", outcome.system_prompt, outcome.has_system_prompt);
        auto message = response.find("message");
        if (message != response.end() && message->is_object() && message->contains("content") &&
            (*message)["content"].is_string()) {
          outcome.messages.push_back(*message);
        }
      } else if (event == HookEvent::before_provider_headers) {
        auto headers = response.find("headers");
        if (headers != response.end() && headers->is_object()) {
          if (!outcome.has_headers) {
            outcome.headers = payload.value("headers", json::object());
          }
          for (auto& [key, value] : headers->items()) {
            for (auto existing = outcome.headers.begin(); existing != outcome.headers.end();) {
              if (niminal::lower_copy(existing.key()) == niminal::lower_copy(key)) {
                existing = outcome.headers.erase(existing);
              } else {
                ++existing;
              }
            }
            if (value.is_string()) {
              outcome.headers[key] = value;
            }
          }
          outcome.has_headers = true;
          payload["headers"] = outcome.headers;
        }
      } else if (event == HookEvent::before_provider_request) {
        copy_object(response, "payload", outcome.payload, outcome.has_payload);
      } else if (event == HookEvent::session_before_compact) {
        if (auto instruction = response.find("instruction");
            instruction != response.end() && instruction->is_string()) {
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
    } catch (const Cancelled&) {
      continue; // pressing escape is not an extension failure
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
  for (const auto& name : impl_->provider_names) {
    niminal::app::unregister_provider(name);
  }
  impl_->provider_names.clear();
  impl_->processes.clear();
  commands_.clear();
  impl_->tools.clear();
  impl_->external_tools.clear();
}

std::vector<ExtensionNotice> ExtensionRuntime::take_notices() {
  return take_values(impl_->actions_mutex, impl_->notices);
}

std::vector<ExtensionUserMessage> ExtensionRuntime::take_user_messages() {
  return take_values(impl_->actions_mutex, impl_->user_messages);
}

std::vector<ExtensionEntry> ExtensionRuntime::take_entries() {
  return take_values(impl_->actions_mutex, impl_->entries);
}

std::vector<ExtensionStatus> ExtensionRuntime::statuses() const {
  std::lock_guard lock(impl_->actions_mutex);
  return copy_values(impl_->statuses);
}

std::vector<ExtensionWidget> ExtensionRuntime::widgets() const {
  std::lock_guard lock(impl_->actions_mutex);
  return copy_values(impl_->widgets);
}

bool ExtensionRuntime::activate_widget_action(const std::string& extension, const std::string& key,
                                              const std::string& action) {
  {
    std::lock_guard lock(impl_->actions_mutex);
    const auto widget = impl_->widgets.find(extension + ":" + key);
    if (widget == impl_->widgets.end() ||
        std::none_of(widget->second.actions.begin(), widget->second.actions.end(),
                     [&](const auto& registered) { return registered.id == action; })) {
      return false;
    }
  }
  const auto process =
      std::find_if(impl_->processes.begin(), impl_->processes.end(),
                   [&](const auto& candidate) { return candidate->name == extension; });
  if (process == impl_->processes.end()) {
    return false;
  }
  try {
    (*process)->send(json{{"type", "ui_action"}, {"widget", key}, {"action", action}});
    return true;
  } catch (...) {
    return false;
  }
}

json session_hook_payload(const std::string& session_id, const fs::path& workspace) {
  return json{{"session_id", session_id}, {"workspace", workspace.string()}};
}

json provider_hook_payload(const niminal::Agent& agent, const fs::path& workspace,
                           const Config& cfg) {
  auto payload = session_hook_payload(agent.conversation_id, workspace);
  payload["provider"] = cfg.provider;
  payload["model"] = agent.model;
  return payload;
}

void bind_extensions(niminal::Agent& agent, const std::shared_ptr<ExtensionRuntime>& runtime,
                     const fs::path& workspace, const Config& cfg,
                     const std::function<void(const std::string&)>& note, Session* session) {
  if (runtime) {
    const std::weak_ptr<ExtensionRuntime> weak_runtime = runtime;
    runtime->set_host_request([&agent, session, weak_runtime, &cfg](const std::string& method,
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
        auto max_tokens = request.value("max_tokens", 4096);
        completion.max_tokens = max_tokens > 0 ? max_tokens : 4096;
        completion.conversation_id = (session ? session->id : agent.conversation_id) + ":extension";
        completion.on_event = {};
        const auto text = niminal::complete_chat(completion);
        return json{{"text", text}, {"model", agent.model}, {"finish_reason", "stop"}};
      }
      if (method == "ui.editor") {
        auto locked = weak_runtime.lock();
        if (!locked) {
          throw std::runtime_error("extension host is unavailable");
        }
        return json{{"text", locked->edit_text(request.value("title", std::string()),
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
  auto dispatch = [runtime, report](HookEvent event, json&& payload) {
    if (!runtime) {
      return HookOutcome{};
    }
    auto outcome = runtime->dispatch(event, std::move(payload));
    report(outcome);
    return outcome;
  };
  agent.before_tool = [dispatch](const niminal::ToolCall& call, json& args, std::string& reason) {
    auto outcome = dispatch(HookEvent::tool_call, json{{"tool", call.name}, {"arguments", args}});
    if (outcome.has_arguments) {
      args = std::move(outcome.arguments);
    }
    reason = outcome.reason;
    return outcome.allowed;
  };
  agent.after_tool = [dispatch](const niminal::ToolCall& call, const json& args,
                                std::string& output, bool& is_error) {
    auto outcome = dispatch(
        HookEvent::tool_result,
        json{{"tool", call.name}, {"arguments", args}, {"output", output}, {"is_error", is_error}});
    if (outcome.has_output) {
      output = std::move(outcome.output);
    }
    if (outcome.has_is_error) {
      is_error = outcome.is_error;
    }
  };
  agent.augment_context = [dispatch](json& messages) {
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
        dispatch(HookEvent::context, json{{"system", system}, {"messages", conversation}});
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
  agent.turn_start = [dispatch, workspace, &agent] {
    dispatch(HookEvent::turn_start, session_hook_payload(agent.conversation_id, workspace));
  };
  agent.turn_end = [dispatch, workspace, &agent](bool interrupted) {
    auto payload = session_hook_payload(agent.conversation_id, workspace);
    if (interrupted) {
      payload["interrupted"] = true;
    }
    dispatch(HookEvent::turn_end, std::move(payload));
  };
  agent.input_hook = [dispatch, workspace, &agent](niminal::UserInput& input) {
    auto payload = session_hook_payload(agent.conversation_id, workspace);
    payload["text"] = input.text;
    payload["images"] = input.images;
    auto outcome = dispatch(HookEvent::input, std::move(payload));
    if (!outcome.allowed) {
      throw niminal::Error(outcome.reason);
    }
    if (outcome.has_text) {
      input.text = std::move(outcome.text);
    }
  };
  agent.before_agent_start = [dispatch, workspace, &agent](const niminal::UserInput& input,
                                                           std::string& system_prompt,
                                                           json& message) {
    auto payload = session_hook_payload(agent.conversation_id, workspace);
    payload["prompt"] = input.text;
    payload["images"] = input.images;
    payload["system_prompt"] = system_prompt;
    auto outcome = dispatch(HookEvent::before_agent_start, std::move(payload));
    if (outcome.has_system_prompt) {
      system_prompt = std::move(outcome.system_prompt);
    }
    for (const auto& injected : outcome.messages) {
      message.push_back(json{{"role", "user"}, {"content", injected["content"]}});
    }
  };
  agent.message_end = [dispatch, workspace, &agent](json& message) {
    auto payload = session_hook_payload(agent.conversation_id, workspace);
    payload["role"] = "assistant";
    payload["text"] = message.value("content", "");
    auto outcome = dispatch(HookEvent::message_end, std::move(payload));
    if (outcome.has_text) {
      message["content"] = std::move(outcome.text);
    }
  };
  agent.agent_settled = [dispatch, workspace, &agent] {
    dispatch(HookEvent::agent_settled, session_hook_payload(agent.conversation_id, workspace));
  };
  agent.before_provider_headers = [dispatch, workspace, &agent,
                                   &cfg](std::map<std::string, std::string>& headers) {
    auto payload = provider_hook_payload(agent, workspace, cfg);
    payload["headers"] = headers;
    auto outcome = dispatch(HookEvent::before_provider_headers, std::move(payload));
    if (outcome.has_headers) {
      headers.clear();
      for (auto& [key, value] : outcome.headers.items()) {
        if (value.is_string()) {
          headers[key] = value.get<std::string>();
        }
      }
    }
  };
  agent.before_provider_request = [dispatch, workspace, &agent, &cfg](json& payload) {
    auto request = provider_hook_payload(agent, workspace, cfg);
    request["payload"] = payload;
    auto outcome = dispatch(HookEvent::before_provider_request, std::move(request));
    if (outcome.has_payload) {
      payload = std::move(outcome.payload);
    }
  };
  agent.after_provider_response = [dispatch, workspace, &agent,
                                   &cfg](const niminal::ProviderResponse& response) {
    auto payload = provider_hook_payload(agent, workspace, cfg);
    payload["status"] = response.status;
    payload["headers"] = response.headers;
    payload["duration_ms"] = response.duration_ms;
    payload["error_body"] = response.status >= 400 ? response.body : std::string();
    dispatch(HookEvent::after_provider_response, std::move(payload));
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
