#include "workspace.hpp"

#include <algorithm>
#include <chrono>
#include <cstdio>
#include <optional>
#include <sstream>
#include <system_error>
#include <utility>
#include <vector>

namespace niminal::app {
namespace fs = std::filesystem;

Workspace::Workspace(const fs::path& root)
    : root_(fs::weakly_canonical(fs::absolute(std::move(root)))) {}

fs::path Workspace::resolve(std::string_view path) const {
  if (path.empty()) {
    throw WorkspaceError("path must not be empty");
  }

  fs::path abs = fs::path(path).is_absolute() ? fs::path(path) : root_ / path;
  abs = abs.lexically_normal();

  fs::path probe = abs;
  std::vector<fs::path> suffix;
  while (!probe.empty() && !fs::exists(probe)) {
    auto name = probe.filename();
    auto parent = probe.parent_path();
    if (parent == probe) {
      break;
    }
    suffix.push_back(std::move(name));
    probe = std::move(parent);
  }

  fs::path real = fs::exists(probe) ? fs::canonical(probe) : abs.lexically_normal();
  for (auto it = suffix.rbegin(); it != suffix.rend(); ++it) {
    real /= *it;
  }

  auto rel = real.lexically_relative(root_);
  if (rel.empty() || rel.native().starts_with("..")) {
    throw WorkspaceError("path is outside the workspace: " + std::string(path));
  }
  return real;
}

std::string Workspace::relative(const fs::path& path) const {
  return path.lexically_relative(root_).generic_string();
}

std::string Workspace::file_version(const fs::path& path) const {
  std::error_code ec;
  auto size = fs::file_size(path, ec);
  if (ec) {
    return {};
  }
  auto stamp = fs::last_write_time(path, ec);
  if (ec) {
    return {};
  }
  auto ns = std::chrono::duration_cast<std::chrono::nanoseconds>(stamp.time_since_epoch()).count();
  std::ostringstream out;
  out << size << ':' << ns;
  return out.str();
}

namespace {

constexpr const char* kSkipDirs[] = {
    ".git", "node_modules",      "build",      ".cache",   "dist",   "target", ".next",
    "out",  "cmake-build-debug", "nimbledeps", "nimcache", ".turbo",
};

bool skip_dir_name(const std::string& name) {
  for (auto d : kSkipDirs) {
    if (name == d) {
      return true;
    }
  }
  return false;
}

bool contains_skipped_dir(const fs::path& path) {
  for (const auto& part : path) {
    if (skip_dir_name(part.string())) {
      return true;
    }
  }
  return false;
}

std::string shell_quote(const std::string& s) {
  std::string out = "'";
  for (char c : s) {
    if (c == '\'') {
      out += "'\\''";
    } else {
      out += c;
    }
  }
  out += "'";
  return out;
}

std::optional<fs::path> git_root_of(fs::path dir) {
  std::error_code ec;
  while (true) {
    if (fs::exists(dir / ".git", ec)) {
      return dir;
    }
    auto parent = dir.parent_path();
    if (parent == dir) {
      return std::nullopt;
    }
    dir = std::move(parent);
  }
}

std::vector<std::string> git_ls_files(const fs::path& git_root) {
  std::string cmd = "git -C " + shell_quote(git_root.string()) +
                    " ls-files -co --exclude-standard -- . 2>/dev/null";
  FILE* pipe = popen(cmd.c_str(), "r");
  if (pipe == nullptr) {
    return {};
  }
  std::vector<std::string> files;
  char buf[4096];
  std::string line;
  while (fgets(buf, sizeof(buf), pipe) != nullptr) {
    line.append(buf);
    if (line.empty() || line.back() != '\n') {
      continue;
    }
    line.pop_back();
    if (!line.empty() && line.back() == '\r') {
      line.pop_back();
    }
    if (!line.empty()) {
      files.push_back(line);
    }
    line.clear();
  }
  if (!line.empty()) {
    files.push_back(line);
  }
  pclose(pipe);
  return files;
}

std::vector<std::string> walk_files(const fs::path& root) {
  std::vector<std::string> files;
  std::error_code ec;
  auto it =
      fs::recursive_directory_iterator(root, fs::directory_options::skip_permission_denied, ec);
  auto end = fs::recursive_directory_iterator();
  for (; it != end; it.increment(ec)) {
    if (ec) {
      ec.clear();
      continue;
    }
    if (it->is_directory(ec) && skip_dir_name(it->path().filename().string())) {
      it.disable_recursion_pending();
      continue;
    }
    if (!it->is_regular_file(ec)) {
      continue;
    }
    files.push_back(it->path().lexically_relative(root).generic_string());
    if (files.size() >= 8000) {
      break;
    }
  }
  return files;
}

} // namespace

void Workspace::invalidate_listing() {
  std::lock_guard<std::mutex> lock(files_mu_);
  files_cached_ = false;
  files_.clear();
}

std::vector<std::string> Workspace::list_files() const {
  std::lock_guard<std::mutex> lock(files_mu_);
  if (files_cached_) {
    return files_;
  }
  std::vector<std::string> files;
  if (auto git_root = git_root_of(root_)) {
    auto listed = git_ls_files(*git_root);
    auto prefix = root_.lexically_relative(*git_root).generic_string();
    if (prefix == ".") {
      prefix.clear();
    }
    for (const auto& file : listed) {
      std::string rel = file;
      if (!prefix.empty()) {
        if (file == prefix) {
          continue;
        }
        auto head = prefix + '/';
        if (!file.starts_with(head)) {
          continue;
        }
        rel = file.substr(head.size());
      }
      if (rel.empty()) {
        continue;
      }
      if (contains_skipped_dir(rel)) {
        continue;
      }
      files.push_back(std::move(rel));
      if (files.size() >= 8000) {
        break;
      }
    }
  } else {
    files = walk_files(root_);
  }
  std::sort(files.begin(), files.end());
  files_ = files;
  files_cached_ = true;
  return files;
}

} // namespace niminal::app
