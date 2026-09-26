#include "tools.hpp"
#include "images.hpp"
#include "instructions.hpp"

#include <algorithm>
#include <atomic>
#include <cerrno>
#include <chrono>
#include <csignal>
#include <cstring>
#include <fcntl.h>
#include <fstream>
#include <functional>
#include <map>
#include <poll.h>
#include <regex>
#include <sstream>
#include <string_view>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

extern char** environ;

namespace niminal::app {
namespace fs = std::filesystem;

using niminal::json;
using niminal::Tool;

namespace {

constexpr size_t kGrepMaxOutputBytes = 32 * 1024;
constexpr size_t kGrepMaxLineBytes = 2 * 1024;
constexpr size_t kGrepNoticeReserveBytes = 160;

bool looks_binary(const fs::path& path) {
  std::ifstream in(path, std::ios::binary);
  if (!in) {
    return true;
  }
  char buf[4096];
  in.read(buf, sizeof(buf));
  auto n = in.gcount();
  return std::find(buf, buf + n, '\0') != buf + n;
}

bool glob_match(std::string_view pat, std::string_view str) {
  while (true) {
    if (pat.starts_with("**")) {
      pat.remove_prefix(2);
      if (pat.starts_with('/')) {
        pat.remove_prefix(1);
      }
      if (pat.empty()) {
        return true;
      }
      for (size_t i = 0; i <= str.size(); ++i) {
        if (i != 0 && str[i - 1] != '/') {
          continue;
        }
        if (glob_match(pat, str.substr(i))) {
          return true;
        }
      }
      return glob_match(pat, {});
    }
    if (pat.empty()) {
      return str.empty();
    }
    if (pat[0] == '*') {
      pat.remove_prefix(1);
      for (size_t i = 0;; ++i) {
        if (glob_match(pat, str.substr(i))) {
          return true;
        }
        if (i == str.size() || str[i] == '/') {
          return false;
        }
      }
    }
    if (str.empty()) {
      return false;
    }
    if (pat[0] == '?') {
      if (str[0] == '/') {
        return false;
      }
      pat.remove_prefix(1);
      str.remove_prefix(1);
      continue;
    }
    if (pat[0] != str[0]) {
      return false;
    }
    pat.remove_prefix(1);
    str.remove_prefix(1);
  }
}

bool glob_match_str(const std::string& pat, const std::string& str) {
  if (pat.starts_with("**/")) {
    return glob_match(pat, str);
  }
  if (pat.find('/') == std::string::npos) {
    return glob_match(pat, str) || glob_match("**/" + pat, str);
  }
  return glob_match(pat, str);
}

bool path_under(std::string_view rel, std::string prefix) {
  while (!prefix.empty() && prefix.back() == '/') {
    prefix.pop_back();
  }
  if (prefix.empty() || prefix == ".") {
    return true;
  }
  return rel == prefix || rel.starts_with(std::string(prefix) + "/");
}

std::string numbered_read(const fs::path& path, int start_line, int end_line) {
  std::ifstream in(path);
  if (!in) {
    throw WorkspaceError("cannot read " + path.string());
  }
  std::ostringstream out;
  std::string line;
  int n = 0;
  size_t bytes = 0;
  constexpr size_t kMax = 200'000;
  while (std::getline(in, line)) {
    ++n;
    if (n < start_line) {
      continue;
    }
    if (n > end_line) {
      break;
    }
    out << n << '|' << line << '\n';
    bytes += line.size() + 1;
    if (bytes > kMax) {
      out << "[truncated]\n";
      break;
    }
  }
  if (n == 0) {
    return "(empty file)\n";
  }
  if (start_line > n) {
    return "start_line past end of file (" + std::to_string(n) + " lines)\n";
  }
  return out.str();
}

std::string unique_replace(std::string text, const std::string& old_text,
                           const std::string& new_text) {
  if (old_text.empty()) {
    throw WorkspaceError("old_text must not be empty");
  }
  auto pos = text.find(old_text);
  if (pos == std::string::npos) {
    throw WorkspaceError("EDIT_FAILED: old_text was not found exactly");
  }
  if (text.find(old_text, pos + 1) != std::string::npos) {
    throw WorkspaceError("EDIT_FAILED: old_text matches more than once; add surrounding context");
  }
  text.replace(pos, old_text.size(), new_text);
  return text;
}

void append_shell_output(std::string& output, std::string_view chunk, bool& pending_cr) {
  for (char c : chunk) {
    if (pending_cr) {
      pending_cr = false;
      if (c != '\n') {
        const auto pos = output.rfind('\n');
        if (pos == std::string::npos) {
          output.clear();
        } else {
          output.resize(pos + 1);
        }
      }
    }
    if (c == '\r') {
      pending_cr = true;
      continue;
    }
    output.push_back(c);
  }
}

bool cap_shell_output(std::string& output) {
  if (output.size() <= 100'000) {
    return false;
  }
  output.resize(100'000);
  output += "\n[truncated]";
  return true;
}

} // namespace

std::string run_bash(const std::string& command, const fs::path& cwd, int timeout_s,
                     niminal::Cancellation* cancel,
                     const std::function<void(const std::string&)>& on_output,
                     const ShellEnv& env) {
  int out_pipe[2];
  if (pipe(out_pipe) != 0) {
    throw WorkspaceError(std::strerror(errno));
  }
  close_on_exec(out_pipe[0]);
  close_on_exec(out_pipe[1]);

  pid_t pid = fork();
  if (pid < 0) {
    close(out_pipe[0]);
    close(out_pipe[1]);
    throw WorkspaceError(std::strerror(errno));
  }
  if (pid == 0) {
    // Its own process group, so the whole tool tree can be killed at once.
    setpgid(0, 0);
    close(out_pipe[0]);
    dup2(out_pipe[1], STDOUT_FILENO);
    dup2(out_pipe[1], STDERR_FILENO);
    close(out_pipe[1]);
    if (chdir(cwd.c_str()) != 0) {
      _exit(127);
    }
    // Export the composed environment via execve; setenv after fork in a
    // multi-threaded parent is unsafe, so the environment is built here.
    auto env_store = child_environment(env, environ);
    std::vector<char*> argv{const_cast<char*>("sh"), const_cast<char*>("-c"),
                            const_cast<char*>(command.c_str()), nullptr};
    std::vector<char*> envp;
    envp.reserve(env_store.size() + 1);
    for (auto& entry : env_store) {
      envp.push_back(entry.data());
    }
    envp.push_back(nullptr);
    execve("/bin/sh", argv.data(), envp.data());
    _exit(127);
  }
  close(out_pipe[1]);
  fcntl(out_pipe[0], F_SETFL, O_NONBLOCK);
  setpgid(pid, pid);
  // Killing the shell alone orphans whatever it started: an `npm run dev` or
  // `python app.py` outlives the SIGKILL and reparents to init. Kill the tree.
  auto kill_tree = [&] {
    kill(-pid, SIGKILL);
    kill(pid, SIGKILL);
  };

  std::string output;
  bool pending_cr = false;
  auto emit = [&] {
    if (on_output) {
      on_output(output);
    }
  };
  auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(timeout_s);
  bool timed_out = false;
  while (true) {
    auto now = std::chrono::steady_clock::now();
    if (now >= deadline || ((cancel != nullptr) && cancel->requested())) {
      timed_out = (cancel == nullptr) || !cancel->requested();
      kill_tree();
      break;
    }
    auto remain = std::chrono::duration_cast<std::chrono::milliseconds>(deadline - now).count();
    pollfd pfd{out_pipe[0], POLLIN, 0};
    int pr = poll(&pfd, 1, static_cast<int>(std::min<long>(remain, 200)));
    if (pr > 0 && ((pfd.revents & POLLIN) != 0)) {
      char buf[4096];
      bool grew = false;
      while (true) {
        auto n = read(out_pipe[0], buf, sizeof(buf));
        if (n > 0) {
          append_shell_output(output, std::string_view(buf, static_cast<size_t>(n)), pending_cr);
          grew = true;
          if (cap_shell_output(output)) {
            kill_tree();
            timed_out = false;
            emit();
            goto wait_child;
          }
        } else {
          break;
        }
      }
      if (grew) {
        emit();
      }
    }
    if (pr > 0 && ((pfd.revents & (POLLHUP | POLLERR)) != 0)) {
      break;
    }
    int status = 0;
    pid_t got = waitpid(pid, &status, WNOHANG);
    if (got == pid) {
      close(out_pipe[0]);
      int code = WIFEXITED(status) ? WEXITSTATUS(status) : 1;
      std::ostringstream msg;
      msg << output;
      if (!output.empty() && output.back() != '\n') {
        msg << '\n';
      }
      msg << "exit: " << code;
      return msg.str();
    }
  }
wait_child:
  int status = 0;
  waitpid(pid, &status, 0);
  // drain
  char buf[4096];
  while (read(out_pipe[0], buf, sizeof(buf)) > 0) {
  }
  close(out_pipe[0]);
  int code = WIFEXITED(status) ? WEXITSTATUS(status) : 1;
  std::ostringstream msg;
  msg << output;
  if (!output.empty() && output.back() != '\n') {
    msg << '\n';
  }
  if (timed_out) {
    msg << "exit: timeout after " << timeout_s << "s";
  } else if ((cancel != nullptr) && cancel->requested()) {
    msg << "exit: interrupted";
  } else {
    msg << "exit: " << code;
  }
  return msg.str();
}

namespace {

std::string read_file_text(const fs::path& path) {
  std::ifstream in(path);
  if (!in) {
    throw WorkspaceError("cannot read " + path.string());
  }
  std::ostringstream ss;
  ss << in.rdbuf();
  return ss.str();
}

void write_file_text(const fs::path& path, const std::string& content) {
  fs::create_directories(path.parent_path());
  auto tmp = path;
  tmp += ".niminal-tmp";
  {
    std::ofstream out(tmp, std::ios::binary | std::ios::trunc);
    if (!out) {
      throw WorkspaceError("cannot write " + path.string());
    }
    out << content;
  }
  fs::rename(tmp, path);
}

} // namespace

std::vector<Tool> workspace_tools(Workspace& ws, niminal::Cancellation* cancel,
                                  const std::function<void(const std::string&)>& on_bash_output,
                                  const ShellEnvFn* shell_env) {
  std::vector<Tool> tools;

  tools.push_back(
      Tool{"read",
           "Read a workspace text file or image. Text returns numbered lines and a version "
           "token for edit. Use start_line and end_line for focused text inspection.",
           json{{"type", "object"},
                {"properties",
                 {{"path", {{"type", "string"}, {"description", "Path relative to workspace."}}},
                  {"start_line",
                   {{"type", "integer"}, {"description", "One-based first line, inclusive."}}},
                  {"end_line",
                   {{"type", "integer"}, {"description", "One-based last line, inclusive."}}}}},
                {"required", json::array({"path"})}},
           [&ws](const json& input) {
             auto path = ws.resolve(input.at("path").get<std::string>());
             if (!fs::is_regular_file(path)) {
               return ToolResult{std::string("File not found: ") + ws.relative(path)};
             }
             if (image_path(path.string())) {
               ToolResult result{"path: " + ws.relative(path) +
                                 "\n[image: " + path.filename().string() + "]\n"};
               result.images.push_back(read_image(path));
               return result;
             }
             int start = input.value("start_line", 1);
             int end = input.value("end_line", 1'000'000'000);
             std::ostringstream out;
             out << "path: " << ws.relative(path) << '\n';
             out << "version: " << ws.file_version(path) << '\n';
             out << numbered_read(path, start, end);
             auto scoped = load_scoped_instructions(ws.root(), path);
             if (!scoped.empty()) {
               out << '\n' << scoped;
             }
             return ToolResult{out.str()};
           },
           true});

  tools.push_back(
      Tool{"grep",
           "Search workspace file contents with plain text or ECMAScript regex. Use glob to "
           "filter files and path to limit the search to a subdirectory. Results are limited "
           "to 32 KB, with long lines truncated.",
           json{{"type", "object"},
                {"properties",
                 {{"pattern", {{"type", "string"}}},
                  {"glob", {{"type", "string"}}},
                  {"path", {{"type", "string"}}},
                  {"case_insensitive", {{"type", "boolean"}}},
                  {"max_matches", {{"type", "integer"}}}}},
                {"required", json::array({"pattern"})}},
           [&ws](const json& input) {
             auto pattern = input.at("pattern").get<std::string>();
             if (pattern.empty()) {
               return std::string("pattern must not be empty");
             }
             auto glob = input.value("glob", std::string());
             auto sub = input.value("path", std::string());
             bool insensitive = input.value("case_insensitive", false);
             int max_hits = std::clamp(input.value("max_matches", 80), 1, 200);
             std::regex::flag_type flags = std::regex::ECMAScript;
             if (insensitive) {
               flags |= std::regex::icase;
             }
             std::regex rx(pattern, flags);
             std::vector<std::string> hits;
             size_t output_bytes = 0;
             bool output_truncated = false;
             bool match_limit_reached = false;
             auto grep_file = [&](const fs::path& path, const std::string& rel) {
               std::error_code ec;
               auto size = fs::file_size(path, ec);
               if (ec || size > 1'000'000) {
                 return false;
               }
               if (looks_binary(path)) {
                 return false;
               }
               std::ifstream in(path);
               std::string line;
               int n = 0;
               while (std::getline(in, line)) {
                 ++n;
                 if (!std::regex_search(line, rx)) {
                   continue;
                 }
                 auto hit = rel + ":" + std::to_string(n) + ":" + line;
                 if (hit.size() > kGrepMaxLineBytes) {
                   hit.resize(kGrepMaxLineBytes - std::string_view("...[line truncated]").size());
                   hit += "...[line truncated]";
                   output_truncated = true;
                 }
                 const size_t separator_bytes = hits.empty() ? 0 : 1;
                 if (hit.size() + separator_bytes >
                     kGrepMaxOutputBytes - output_bytes - kGrepNoticeReserveBytes) {
                   output_truncated = true;
                   return true;
                 }
                 output_bytes += hit.size() + separator_bytes;
                 hits.push_back(std::move(hit));
                 if (static_cast<int>(hits.size()) >= max_hits) {
                   match_limit_reached = true;
                   return true;
                 }
               }
               return false;
             };
             std::string prefix;
             bool direct_file = false;
             if (!sub.empty()) {
               auto start = ws.resolve(sub);
               if (fs::is_regular_file(start)) {
                 grep_file(start, ws.relative(start));
                 direct_file = true;
               } else {
                 prefix = ws.relative(start);
               }
             }
             if (!direct_file) {
               for (const auto& rel : ws.list_files()) {
                 if (!path_under(rel, prefix) || (!glob.empty() && !glob_match_str(glob, rel))) {
                   continue;
                 }
                 if (grep_file(ws.resolve(rel), rel)) {
                   break;
                 }
               }
             }
             if (hits.empty()) {
               return std::string("No matches.");
             }
             std::ostringstream out;
             for (size_t i = 0; i < hits.size(); ++i) {
               if (i) {
                 out << '\n';
               }
               out << hits[i];
             }
             if (output_truncated) {
               out << "\n[grep output truncated; narrow the pattern or path for more results]";
             }
             if (match_limit_reached) {
               out << "\n[grep stopped at " << max_hits << " matches; results may be incomplete]";
             }
             return out.str();
           },
           true});

  tools.push_back(Tool{
      "glob",
      "Find workspace files matching a glob (e.g. **/*.cpp, src/*). Results honor "
      ".gitignore.",
      json{{"type", "object"},
           {"properties", {{"pattern", {{"type", "string"}}}, {"path", {{"type", "string"}}}}},
           {"required", json::array({"pattern"})}},
      [&ws](const json& input) {
        auto pattern = input.at("pattern").get<std::string>();
        if (pattern.empty()) {
          return std::string("pattern must not be empty");
        }
        auto sub = input.value("path", std::string());
        std::string prefix;
        if (!sub.empty()) {
          auto start = ws.resolve(sub);
          prefix = fs::is_directory(start) ? ws.relative(start) : ws.relative(start.parent_path());
        }
        std::vector<std::string> hits;
        for (const auto& rel : ws.list_files()) {
          if (!path_under(rel, prefix)) {
            continue;
          }
          if (!glob_match_str(pattern, rel)) {
            continue;
          }
          hits.push_back(rel);
          if (hits.size() >= 200) {
            break;
          }
        }
        if (hits.empty()) {
          return std::string("No files.");
        }
        std::ostringstream out;
        for (size_t i = 0; i < hits.size(); ++i) {
          if (i) {
            out << '\n';
          }
          out << hits[i];
        }
        return out.str();
      },
      true});

  tools.push_back(
      Tool{"ls",
           "List one actual directory, including ignored entries and empty directories that "
           "glob and grep skip. Directories end with /.",
           json{{"type", "object"},
                {"properties", {{"path", {{"type", "string"}}}}},
                {"required", json::array()}},
           [&ws](const json& input) {
             auto rel = input.value("path", std::string("."));
             auto dir = ws.resolve(rel);
             std::error_code ec;
             if (!fs::is_directory(dir, ec)) {
               return "Not a directory: " + rel;
             }
             std::vector<std::string> entries;
             for (const auto& entry : fs::directory_iterator(dir, ec)) {
               entries.push_back(entry.is_directory(ec) ? entry.path().filename().string() + "/"
                                                        : entry.path().filename().string());
             }
             if (entries.empty()) {
               return std::string("Empty directory.");
             }
             std::sort(entries.begin(), entries.end());
             bool truncated = entries.size() > 200;
             if (truncated) {
               entries.resize(200);
             }
             std::ostringstream out;
             for (size_t i = 0; i < entries.size(); ++i) {
               if (i) {
                 out << '\n';
               }
               out << entries[i];
             }
             if (truncated) {
               out << "\n[truncated]";
             }
             return out.str();
           },
           true});

  tools.push_back(Tool{
      "edit",
      "Replace unique old_text with new_text in a file. Read the file first and pass its version "
      "as expected_version when available; use this for targeted changes.",
      json{{"type", "object"},
           {"properties",
            {{"path", {{"type", "string"}}},
             {"old_text", {{"type", "string"}}},
             {"new_text", {{"type", "string"}}},
             {"expected_version",
              {{"type", "string"}, {"description", "Version returned by the latest read."}}}}},
           {"required", json::array({"path", "old_text", "new_text"})}},
      [&ws](const json& input) {
        auto path = ws.resolve(input.at("path").get<std::string>());
        if (!fs::is_regular_file(path)) {
          return std::string("File not found: ") + ws.relative(path);
        }
        if (input.contains("expected_version")) {
          auto expected = input["expected_version"].get<std::string>();
          auto got = ws.file_version(path);
          if (expected != got) {
            return std::string("version mismatch; re-read the file");
          }
        }
        auto next = unique_replace(read_file_text(path), input.at("old_text").get<std::string>(),
                                   input.at("new_text").get<std::string>());
        write_file_text(path, next);
        ws.invalidate_listing();
        return "OK — edited " + ws.relative(path) + "\nversion: " + ws.file_version(path);
      }});

  tools.push_back(
      Tool{"write",
           "Create a new file or replace a file's complete contents. Use edit for targeted "
           "changes.",
           json{{"type", "object"},
                {"properties",
                 {{"path", {{"type", "string"}}},
                  {"content", {{"type", "string"}}},
                  {"overwrite", {{"type", "boolean"}}}}},
                {"required", json::array({"path", "content"})}},
           [&ws](const json& input) {
             auto path = ws.resolve(input.at("path").get<std::string>());
             bool overwrite = input.value("overwrite", false);
             if (fs::exists(path) && !overwrite) {
               return "File already exists: " + ws.relative(path) +
                      "\nSet overwrite: true to replace it.";
             }
             write_file_text(path, input.at("content").get<std::string>());
             ws.invalidate_listing();
             return "OK — wrote " + ws.relative(path) + "\nversion: " + ws.file_version(path);
           }});

  tools.push_back(
      Tool{"bash",
           "Run a shell command in the workspace. Returns combined stdout/stderr and exit code. "
           "Exports the session env (NIMINAL_SESSION_ID, NIMINAL_SESSION_FILE, NIMINAL_PROVIDER, "
           "NIMINAL_MODEL, NIMINAL_REASONING_LEVEL) so hooks and scripts can introspect the "
           "session. Use it for tests, builds, formatters, git, and other shell workflows; shell "
           "utilities may also inspect or transform files when useful. Prefer the structured "
           "workspace tools when their focused behavior is more convenient.",
           json{{"type", "object"},
                {"properties",
                 {{"command", {{"type", "string"}}}, {"timeout_seconds", {{"type", "integer"}}}}},
                {"required", json::array({"command"})}},
           [&ws, cancel, on_bash_output, shell_env](const json& input) {
             auto command = input.at("command").get<std::string>();
             int timeout = std::clamp(input.value("timeout_seconds", 120), 1, 600);
             ShellEnv env = shell_env != nullptr ? (*shell_env)() : ShellEnv{};
             auto out = run_bash(command, ws.root(), timeout, cancel, on_bash_output, env);
             ws.invalidate_listing();
             return out;
           }});

  return tools;
}

ShellEnv make_shell_env(const Session& session, const niminal::Agent& agent, const Config& cfg) {
  ShellEnv env;
  env["NIMINAL_SESSION_ID"] = session.id;
  if (!session.path.empty()) {
    env["NIMINAL_SESSION_FILE"] = session.path;
  }
  env["NIMINAL_PROVIDER"] = cfg.provider;
  env["NIMINAL_MODEL"] = agent.model;
  if (!cfg.thinking.empty()) {
    env["NIMINAL_REASONING_LEVEL"] = cfg.thinking;
  }
  return env;
}

} // namespace niminal::app
