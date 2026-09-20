#include "mentions.hpp"

#include <algorithm>
#include <cctype>
#include <fstream>
#include <iterator>

namespace niminal::app {
namespace {

constexpr size_t kAttachmentLimit = 100'000;

bool mention_char(char c) {
  return std::isalnum(static_cast<unsigned char>(c)) || c == '_' || c == '.' ||
         c == '/' || c == '-' || c == '+';
}

std::string lower(std::string value) {
  std::transform(value.begin(), value.end(), value.begin(), [](unsigned char c) {
    return static_cast<char>(std::tolower(c));
  });
  return value;
}

std::string basename(std::string_view path) {
  auto slash = path.find_last_of('/');
  return std::string(path.substr(slash == std::string_view::npos ? 0 : slash + 1));
}

}  // namespace

std::optional<FileMention> file_mention_at(std::string_view text, size_t cursor) {
  cursor = std::min(cursor, text.size());
  size_t token = cursor;
  while (token > 0 && mention_char(text[token - 1])) --token;
  if (token == 0 || text[token - 1] != '@') return std::nullopt;
  size_t start = token - 1;
  if (start > 0 && !std::isspace(static_cast<unsigned char>(text[start - 1])))
    return std::nullopt;
  size_t end = token;
  while (end < text.size() && mention_char(text[end])) ++end;
  return FileMention{start, end, std::string(text.substr(token, cursor - token))};
}

std::vector<std::string> suggest_mentioned_files(const Workspace& workspace,
                                                 std::string_view query,
                                                 size_t limit) {
  auto q = lower(std::string(query));
  auto files = workspace.list_files();
  std::vector<std::string> out;
  auto add = [&](bool basename_match) {
    for (const auto& file : files) {
      auto path = lower(file);
      auto name = lower(basename(file));
      bool match = q.empty() || (basename_match ? name.starts_with(q)
                                                : path.find(q) != std::string::npos);
      if (!match || std::find(out.begin(), out.end(), file) != out.end()) continue;
      out.push_back(file);
      if (out.size() == limit) return;
    }
  };
  add(true);
  if (!q.empty() && out.size() < limit) add(false);
  return out;
}

std::string apply_file_mention(std::string_view text, size_t cursor,
                               std::string_view path) {
  auto mention = file_mention_at(text, cursor);
  if (!mention) return std::string(text);
  return std::string(text.substr(0, mention->start)) + '@' + std::string(path) +
         ' ' + std::string(text.substr(mention->end));
}

std::string expand_file_mentions(const Workspace& workspace,
                                 std::string_view prompt) {
  std::string attachments;
  for (size_t i = 0; i < prompt.size();) {
    if (prompt[i] != '@' ||
        (i > 0 && !std::isspace(static_cast<unsigned char>(prompt[i - 1])))) {
      ++i;
      continue;
    }
    size_t end = i + 1;
    while (end < prompt.size() && mention_char(prompt[end])) ++end;
    if (end == i + 1) {
      ++i;
      continue;
    }
    auto relative = std::string(prompt.substr(i + 1, end - i - 1));
    try {
      auto path = workspace.resolve(relative);
      if (!std::filesystem::is_regular_file(path)) {
        i = end;
        continue;
      }
      std::ifstream in(path, std::ios::binary);
      std::string body((std::istreambuf_iterator<char>(in)), {});
      if (body.substr(0, std::min<size_t>(body.size(), 4096)).find('\0') !=
          std::string::npos) {
        i = end;
        continue;
      }
      bool truncated = body.size() > kAttachmentLimit;
      body.resize(std::min(body.size(), kAttachmentLimit));
      attachments += "\n<file path=\"" + workspace.relative(path) + "\">\n" + body;
      if (body.empty() || body.back() != '\n') attachments += '\n';
      if (truncated) attachments += "[truncated]\n";
      attachments += "</file>\n";
    } catch (const std::exception&) {
    }
    i = end;
  }
  return std::string(prompt) + attachments;
}

}  // namespace niminal::app
