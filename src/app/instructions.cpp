#include "instructions.hpp"
#include "config.hpp"

#include <algorithm>
#include <fstream>
#include <mutex>
#include <sstream>
#include <system_error>
#include <vector>

namespace niminal::app {
namespace fs = std::filesystem;

namespace {

constexpr size_t kMaxBytes = 64 * 1024;

struct CacheEntry {
  std::vector<fs::path> paths;
  std::vector<fs::file_time_type> mtimes;
  std::string text;
};

std::mutex cache_mu;
std::vector<CacheEntry> cache;

fs::path context_file(const fs::path& dir) {
  std::error_code ec;
  auto override_p = dir / "AGENTS.override.md";
  if (fs::is_regular_file(override_p, ec)) {
    return override_p;
  }
  auto agents = dir / "AGENTS.md";
  if (fs::is_regular_file(agents, ec)) {
    return agents;
  }
  auto claude = dir / "CLAUDE.md";
  if (fs::is_regular_file(claude, ec)) {
    return claude;
  }
  return {};
}

bool is_git_root(const fs::path& dir) {
  std::error_code ec;
  return fs::exists(dir / ".git", ec);
}

std::string read_bounded(const fs::path& path) {
  std::ifstream in(path);
  if (!in) {
    return {};
  }
  std::ostringstream ss;
  ss << in.rdbuf();
  auto text = ss.str();
  if (text.size() > kMaxBytes) {
    text.resize(kMaxBytes);
    text += "\n\n[instructions truncated]\n";
  }
  return text;
}

std::vector<fs::file_time_type> mtimes_of(const std::vector<fs::path>& paths) {
  std::vector<fs::file_time_type> out;
  out.reserve(paths.size());
  for (const auto& path : paths) {
    std::error_code ec;
    auto t = fs::last_write_time(path, ec);
    out.push_back(ec ? fs::file_time_type{} : t);
  }
  return out;
}

std::string format_instructions(const fs::path& workspace, const fs::path& global,
                                const std::vector<fs::path>& paths) {
  std::ostringstream out;
  out << "Project instructions. Apply less-specific files before more-specific "
         "files:\n";
  bool any = false;
  for (const auto& path : paths) {
    auto content = read_bounded(path);
    if (content.empty()) {
      continue;
    }
    any = true;
    std::string label =
        (path == global) ? "global" : path.lexically_relative(workspace).generic_string();
    if (label.empty() || label.starts_with("..")) {
      label = path.filename().string();
    }
    out << "\n<file path=\"" << label << "\">\n" << content;
    if (content.back() != '\n') {
      out << '\n';
    }
    out << "</file>\n";
  }
  return any ? out.str() : std::string();
}

std::string load_cached(const fs::path& workspace, const fs::path& global,
                        const std::vector<fs::path>& paths) {
  if (paths.empty()) {
    return {};
  }
  auto mtimes = mtimes_of(paths);
  std::lock_guard lock(cache_mu);
  for (const auto& entry : cache) {
    if (entry.paths == paths && entry.mtimes == mtimes) {
      return entry.text;
    }
  }
  auto text = format_instructions(workspace, global, paths);
  cache.push_back({paths, std::move(mtimes), text});
  return text;
}

} // namespace

fs::path global_agents_path() {
  return context_file(config_path().parent_path());
}

std::vector<fs::path> instruction_paths(const fs::path& workspace) {
  std::vector<fs::path> paths;
  auto global = global_agents_path();
  if (!global.empty()) {
    paths.push_back(global);
  }

  std::error_code ec;
  auto current = fs::weakly_canonical(fs::absolute(workspace), ec);
  if (ec) {
    current = fs::absolute(workspace);
  }
  auto stop = current;
  {
    auto probe = current;
    while (true) {
      if (is_git_root(probe)) {
        stop = probe;
        break;
      }
      auto parent = probe.parent_path();
      if (parent == probe) {
        stop = current;
        break;
      }
      probe = parent;
    }
  }
  std::vector<fs::path> project;
  while (true) {
    auto found = context_file(current);
    if (!found.empty()) {
      project.push_back(found);
    }
    if (current == stop) {
      break;
    }
    auto parent = current.parent_path();
    if (parent == current) {
      break;
    }
    current = parent;
  }
  std::reverse(project.begin(), project.end());
  paths.insert(paths.end(), project.begin(), project.end());
  return paths;
}

std::string load_project_instructions(const fs::path& workspace) {
  auto paths = instruction_paths(workspace);
  return load_cached(workspace, global_agents_path(), paths);
}

std::string load_scoped_instructions(const fs::path& workspace, const fs::path& target) {
  auto skip = instruction_paths(workspace);
  std::error_code ec;
  auto root = fs::weakly_canonical(fs::absolute(workspace), ec);
  if (ec) {
    root = fs::absolute(workspace);
  }
  auto current = fs::weakly_canonical(fs::absolute(target), ec);
  if (ec) {
    current = fs::absolute(target);
  }
  if (fs::is_regular_file(current, ec)) {
    current = current.parent_path();
  }

  std::vector<fs::path> found;
  while (!current.empty()) {
    auto rel = current.lexically_relative(root);
    if (rel.empty() || rel.native().starts_with("..")) {
      break;
    }
    if (current == root) {
      break;
    }
    auto path = context_file(current);
    if (!path.empty() && std::find(skip.begin(), skip.end(), path) == skip.end()) {
      found.push_back(path);
    }
    auto parent = current.parent_path();
    if (parent == current) {
      break;
    }
    current = parent;
  }
  std::reverse(found.begin(), found.end());
  if (found.empty()) {
    return {};
  }
  std::ostringstream out;
  out << "Instructions for the requested path:\n";
  bool any = false;
  for (const auto& path : found) {
    auto content = read_bounded(path);
    if (content.empty()) {
      continue;
    }
    any = true;
    auto label = path.lexically_relative(root).generic_string();
    out << "\n<file path=\"" << label << "\">\n" << content;
    if (content.back() != '\n') {
      out << '\n';
    }
    out << "</file>\n";
  }
  return any ? out.str() : std::string();
}

} // namespace niminal::app
