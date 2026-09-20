#include "prompts.hpp"
#include "config.hpp"

#include <algorithm>
#include <cctype>
#include <fstream>
#include <map>
#include <sstream>
#include <utility>

namespace niminal::app {
namespace fs = std::filesystem;

namespace {

constexpr size_t kMaxPromptBytes = 100'000;

std::string lower_copy(std::string value) {
  for (char& c : value)
    if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
  return value;
}

std::string trim_copy(std::string value) {
  auto is_space = [](unsigned char c) { return std::isspace(c) != 0; };
  while (!value.empty() && is_space(static_cast<unsigned char>(value.back())))
    value.pop_back();
  size_t first = 0;
  while (first < value.size() && is_space(static_cast<unsigned char>(value[first])))
    ++first;
  return value.substr(first);
}

std::string unquote(std::string value) {
  value = trim_copy(std::move(value));
  if (value.size() >= 2 &&
      ((value.front() == '"' && value.back() == '"') ||
       (value.front() == '\'' && value.back() == '\'')))
    return value.substr(1, value.size() - 2);
  return value;
}

std::string read_file(const fs::path& path) {
  std::ifstream in(path, std::ios::binary);
  if (!in) return {};
  std::ostringstream out;
  out << in.rdbuf();
  return out.str();
}

PromptTemplate parse_template(const fs::path& path) {
  PromptTemplate result;
  result.name = path.stem().string();
  result.path = path;
  auto text = read_file(path);
  if (text.empty() || text.size() > kMaxPromptBytes) return {};

  std::istringstream input(text);
  std::vector<std::string> lines;
  std::string line;
  while (std::getline(input, line)) {
    if (!line.empty() && line.back() == '\r') line.pop_back();
    lines.push_back(std::move(line));
  }

  size_t body_at = 0;
  if (!lines.empty() && trim_copy(lines[0]) == "---") {
    body_at = 1;
    bool closed = false;
    for (; body_at < lines.size(); ++body_at) {
      auto stripped = trim_copy(lines[body_at]);
      if (stripped == "---") {
        ++body_at;
        closed = true;
        break;
      }
      auto colon = stripped.find(':');
      if (colon == std::string::npos || colon == 0) continue;
      if (lower_copy(trim_copy(stripped.substr(0, colon))) == "description")
        result.description = unquote(stripped.substr(colon + 1));
    }
    if (!closed) return {};
  }

  for (size_t i = body_at; i < lines.size(); ++i) {
    if (i != body_at) result.body.push_back('\n');
    result.body += lines[i];
  }
  result.body = trim_copy(std::move(result.body));
  if (result.description.empty()) {
    for (size_t i = body_at; i < lines.size(); ++i) {
      auto candidate = trim_copy(lines[i]);
      if (!candidate.empty()) {
        result.description = std::move(candidate);
        break;
      }
    }
  }
  if (result.body.empty()) return {};
  return result;
}

std::vector<fs::path> roots_for(const fs::path& workspace) {
  std::vector<fs::path> roots;
  try {
    auto global = config_path().parent_path();
    roots.push_back(global.parent_path() / ".agents" / "prompts");
    roots.push_back(global / "prompts");
  } catch (...) {
  }
  auto root = fs::absolute(workspace).lexically_normal();
  roots.push_back(root / ".agent" / "prompts");
  roots.push_back(root / ".agents" / "prompts");
  roots.push_back(root / ".niminal" / "prompts");
  return roots;
}

void add_dir(std::map<std::string, PromptTemplate>& found, const fs::path& dir) {
  std::error_code ec;
  if (!fs::is_directory(dir, ec)) return;
  std::vector<fs::path> paths;
  for (const auto& entry : fs::directory_iterator(dir, ec)) {
    if (entry.is_regular_file(ec) &&
        lower_copy(entry.path().extension().string()) == ".md")
      paths.push_back(entry.path());
  }
  std::sort(paths.begin(), paths.end());
  for (const auto& path : paths) {
    auto prompt = parse_template(path);
    if (!prompt.name.empty() && !prompt.body.empty())
      found[lower_copy(prompt.name)] = std::move(prompt);
  }
}

std::string replace_all(std::string text, const std::string& from,
                        const std::string& to) {
  size_t pos = 0;
  while ((pos = text.find(from, pos)) != std::string::npos) {
    text.replace(pos, from.size(), to);
    pos += to.size();
  }
  return text;
}

}  // namespace

std::vector<PromptTemplate> discover_prompts(const fs::path& workspace) {
  std::map<std::string, PromptTemplate> found;
  for (const auto& root : roots_for(workspace)) add_dir(found, root);
  std::vector<PromptTemplate> result;
  for (auto& [_, prompt] : found) result.push_back(std::move(prompt));
  std::sort(result.begin(), result.end(), [](const PromptTemplate& a,
                                             const PromptTemplate& b) {
    auto an = lower_copy(a.name);
    auto bn = lower_copy(b.name);
    return an == bn ? a.path < b.path : an < bn;
  });
  return result;
}

std::optional<PromptTemplate> load_prompt(const fs::path& workspace,
                                          const std::string& name) {
  auto wanted = lower_copy(name);
  for (auto& prompt : discover_prompts(workspace))
    if (lower_copy(prompt.name) == wanted) return prompt;
  return std::nullopt;
}

std::string expand_prompt(const fs::path& workspace, const std::string& name,
                          const std::string& arguments) {
  auto prompt = load_prompt(workspace, name);
  if (!prompt) return {};
  return replace_all(replace_all(prompt->body, "$ARGUMENTS", arguments), "$@",
                     arguments);
}

}  // namespace niminal::app
