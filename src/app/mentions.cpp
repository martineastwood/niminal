#include "mentions.hpp"
#include "images.hpp"

#include <niminal/text.hpp>

#include <algorithm>
#include <cctype>
#include <fstream>
#include <iterator>

namespace niminal::app {
namespace {

constexpr size_t kAttachmentLimit = 100'000;

bool mention_char(char c) {
  return (std::isalnum(static_cast<unsigned char>(c)) != 0) || c == '_' || c == '.' || c == '/' ||
         c == '-' || c == '+';
}

std::string basename(std::string_view path) {
  auto slash = path.find_last_of('/');
  return std::string(path.substr(slash == std::string_view::npos ? 0 : slash + 1));
}

int hex_value(char c) {
  if (c >= '0' && c <= '9') {
    return c - '0';
  }
  if (c >= 'a' && c <= 'f') {
    return c - 'a' + 10;
  }
  if (c >= 'A' && c <= 'F') {
    return c - 'A' + 10;
  }
  return -1;
}

std::string percent_decode(std::string_view in) {
  std::string out;
  out.reserve(in.size());
  for (size_t i = 0; i < in.size();) {
    if (in[i] == '%' && i + 2 < in.size()) {
      const int hi = hex_value(in[i + 1]);
      const int lo = hex_value(in[i + 2]);
      if (hi >= 0 && lo >= 0) {
        out.push_back(static_cast<char>((hi << 4) | lo));
        i += 3;
        continue;
      }
    }
    out.push_back(in[i]);
    ++i;
  }
  return out;
}

std::string normalize_dropped_path(std::string path) {
  if (path.rfind("file://", 0) == 0) {
    path.erase(0, 7);
  }
  return percent_decode(path);
}

struct PathMention {
  size_t start;
  size_t end;
  std::string_view relative;
};

template <typename Fn> void for_each_path_mention(std::string_view text, Fn&& fn) {
  for (size_t i = 0; i < text.size();) {
    if (text[i] != '@' || (i > 0 && (std::isspace(static_cast<unsigned char>(text[i - 1])) == 0))) {
      ++i;
      continue;
    }
    const size_t start = i;
    size_t end = i + 1;
    while (end < text.size() && mention_char(text[end])) {
      ++end;
    }
    if (end == start + 1) {
      ++i;
      continue;
    }
    fn(PathMention{start, end, text.substr(start + 1, end - start - 1)});
    i = end;
  }
}

} // namespace

std::optional<FileMention> file_mention_at(std::string_view text, size_t cursor) {
  cursor = std::min(cursor, text.size());
  size_t token = cursor;
  while (token > 0 && mention_char(text[token - 1])) {
    --token;
  }
  if (token == 0 || text[token - 1] != '@') {
    return std::nullopt;
  }
  size_t start = token - 1;
  if (start > 0 && (std::isspace(static_cast<unsigned char>(text[start - 1])) == 0)) {
    return std::nullopt;
  }
  size_t end = token;
  while (end < text.size() && mention_char(text[end])) {
    ++end;
  }
  return FileMention{start, end, std::string(text.substr(token, cursor - token))};
}

std::vector<std::string> suggest_mentioned_files(const Workspace& workspace, std::string_view query,
                                                 size_t limit) {
  auto q = niminal::lower_copy(std::string(query));
  auto files = workspace.list_files();
  std::vector<std::string> out;
  auto add = [&](bool basename_match) {
    for (const auto& file : files) {
      auto path = niminal::lower_copy(file);
      auto name = niminal::lower_copy(basename(file));
      bool match =
          q.empty() || (basename_match ? name.starts_with(q) : path.find(q) != std::string::npos);
      if (!match || std::find(out.begin(), out.end(), file) != out.end()) {
        continue;
      }
      out.push_back(file);
      if (out.size() == limit) {
        return;
      }
    }
  };
  add(true);
  if (!q.empty() && out.size() < limit) {
    add(false);
  }
  return out;
}

std::string apply_file_mention(std::string_view text, size_t cursor, std::string_view path) {
  auto mention = file_mention_at(text, cursor);
  if (!mention) {
    return std::string(text);
  }
  return std::string(text.substr(0, mention->start)) + '@' + std::string(path) + ' ' +
         std::string(text.substr(mention->end));
}

std::string expand_file_mentions(const Workspace& workspace, std::string_view prompt) {
  std::string attachments;
  for_each_path_mention(prompt, [&](const PathMention& mention) {
    const auto relative = std::string(mention.relative);
    if (image_path(relative)) {
      return;
    }
    try {
      auto path = workspace.resolve(relative);
      if (!std::filesystem::is_regular_file(path)) {
        return;
      }
      std::ifstream in(path, std::ios::binary);
      std::string body((std::istreambuf_iterator<char>(in)), {});
      if (body.substr(0, std::min<size_t>(body.size(), 4096)).find('\0') != std::string::npos) {
        return;
      }
      const bool truncated = body.size() > kAttachmentLimit;
      body.resize(std::min(body.size(), kAttachmentLimit));
      attachments += "\n<file path=\"" + workspace.relative(path) + "\">\n" + body;
      if (body.empty() || body.back() != '\n') {
        attachments += '\n';
      }
      if (truncated) {
        attachments += "[truncated]\n";
      }
      attachments += "</file>\n";
    } catch (const std::exception&) {
    }
  });
  return std::string(prompt) + attachments;
}

niminal::UserInput prepare_user_input(const Workspace& workspace, niminal::UserInput input) {
  for_each_path_mention(input.text, [&](const PathMention& mention) {
    if (!image_path(mention.relative)) {
      return;
    }
    const auto filename = std::filesystem::path(mention.relative).filename().string();
    for (const auto& image : input.images) {
      if (image.value("name", "") == filename) {
        return;
      }
    }
    auto path = workspace.resolve(mention.relative);
    if (!std::filesystem::is_regular_file(path)) {
      throw niminal::Error("image not found: " + std::string(mention.relative));
    }
    input.images.push_back(read_image(path));
  });
  input.text = expand_file_mentions(workspace, input.text);
  if (!input.images.empty()) {
    return input;
  }
  std::string candidate;
  char quote = 0;
  std::vector<std::string> paths;
  auto finish = [&] {
    if (!candidate.empty()) {
      paths.push_back(std::move(candidate));
      candidate.clear();
    }
  };
  for (size_t i = 0; i < input.text.size(); ++i) {
    const char c = input.text[i];
    if (c == '\\' && i + 1 < input.text.size()) {
      candidate += input.text[++i];
    } else if (quote != 0 && c == quote) {
      quote = 0;
    } else if (quote == 0 && (c == '\'' || c == '"')) {
      quote = c;
    } else if (quote == 0 && std::isspace(static_cast<unsigned char>(c)) != 0) {
      finish();
    } else {
      candidate += c;
    }
  }
  finish();
  if (quote != 0 || paths.empty()) {
    return input;
  }
  niminal::json images = niminal::json::array();
  for (const auto& raw : paths) {
    auto path = normalize_dropped_path(raw);
    const auto file = std::filesystem::path(path).is_absolute() ? std::filesystem::path(path)
                                                                : workspace.root() / path;
    if (!image_path(path) || !std::filesystem::is_regular_file(file)) {
      return input;
    }
    images.push_back(read_image(file));
  }
  input.text.clear();
  input.images = std::move(images);
  return input;
}

} // namespace niminal::app
