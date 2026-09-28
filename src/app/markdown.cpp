#include "markdown.hpp"

#include <niminal/text.hpp>

#include <ftxui/dom/flexbox_config.hpp>
#include <ftxui/dom/table.hpp>

#include <algorithm>
#include <cctype>
#include <sstream>
#include <utility>
#include <vector>

namespace niminal::app {
namespace {

using namespace ftxui;

struct Span {
  std::string text;
  bool bold = false;
  bool italic = false;
  bool code = false;
  bool strike = false;
  bool underline = false;
  bool dim = false;
};

bool starts_at(std::string_view s, size_t i, std::string_view p) {
  return i + p.size() <= s.size() && s.substr(i, p.size()) == p;
}

std::vector<Span> parse_inline(std::string_view s, Span base = {}) {
  if (s.find_first_of("*_~`[!") == std::string_view::npos) {
    base.text = s;
    return {std::move(base)};
  }
  std::vector<Span> out;
  std::string acc;
  auto flush = [&] {
    if (acc.empty()) {
      return;
    }
    Span span = base;
    span.text = std::move(acc);
    out.push_back(std::move(span));
    acc.clear();
  };
  size_t i = 0;
  while (i < s.size()) {
    auto try_delimited = [&](std::string_view marker, auto apply, bool word_boundary = false) {
      if (!starts_at(s, i, marker)) {
        return false;
      }
      if (word_boundary && i > 0 && std::isalnum(static_cast<unsigned char>(s[i - 1]))) {
        return false;
      }
      const auto inner_start = i + marker.size();
      if (word_boundary &&
          (inner_start >= s.size() || s[inner_start] == ' ' || s[inner_start] == '\t')) {
        return false;
      }
      const auto close = s.find(marker, inner_start);
      if (close == std::string_view::npos ||
          (word_boundary &&
           (close == inner_start || s[close - 1] == ' ' || s[close - 1] == '\t' ||
            (close + marker.size() < s.size() &&
             std::isalnum(static_cast<unsigned char>(s[close + marker.size()])))))) {
        return false;
      }
      flush();
      Span inner = base;
      apply(inner);
      auto kids = parse_inline(s.substr(inner_start, close - inner_start), inner);
      out.insert(out.end(), kids.begin(), kids.end());
      i = close + marker.size();
      return true;
    };
    if (try_delimited("***", [](Span& sp) { sp.bold = sp.italic = true; })) {
      continue;
    }
    if (try_delimited("**", [](Span& sp) { sp.bold = true; })) {
      continue;
    }
    if (try_delimited("~~", [](Span& sp) { sp.strike = true; })) {
      continue;
    }
    if (try_delimited("___", [](Span& sp) { sp.bold = sp.italic = true; }, true)) {
      continue;
    }
    if (try_delimited("__", [](Span& sp) { sp.bold = true; }, true)) {
      continue;
    }
    if (starts_at(s, i, "`")) {
      auto close = s.find('`', i + 1);
      if (close != std::string_view::npos) {
        flush();
        Span inner = base;
        inner.code = true;
        inner.text = std::string(s.substr(i + 1, close - (i + 1)));
        out.push_back(std::move(inner));
        i = close + 1;
        continue;
      }
    }
    if (try_delimited("*", [](Span& sp) { sp.italic = true; })) {
      continue;
    }
    if (try_delimited("_", [](Span& sp) { sp.italic = true; }, true)) {
      continue;
    }
    bool image = s[i] == '!' && i + 1 < s.size() && s[i + 1] == '[';
    if (s[i] == '[' || image) {
      auto bracket = image ? i + 1 : i;
      auto close = s.find(']', bracket + 1);
      if (close != std::string_view::npos && close + 1 < s.size() && s[close + 1] == '(') {
        auto finish = s.find(')', close + 2);
        if (finish != std::string_view::npos) {
          flush();
          auto label = s.substr(bracket + 1, close - (bracket + 1));
          auto url = s.substr(close + 2, finish - (close + 2));
          if (image) {
            Span alt = base;
            alt.italic = true;
            alt.text = label.empty() ? "[image]" : "[" + std::string(label) + "]";
            out.push_back(std::move(alt));
            if (!url.empty()) {
              Span dest = base;
              dest.dim = true;
              dest.text = " (" + std::string(url) + ")";
              out.push_back(std::move(dest));
            }
          } else {
            Span styled = base;
            styled.underline = true;
            auto kids = parse_inline(label, styled);
            out.insert(out.end(), kids.begin(), kids.end());
            Span dest = base;
            dest.dim = true;
            dest.text = " (" + std::string(url) + ")";
            out.push_back(std::move(dest));
          }
          i = finish + 1;
          continue;
        }
      }
    }
    acc += s[i];
    ++i;
  }
  flush();
  return out;
}

std::string trim(std::string_view s) {
  size_t a = 0;
  while (a < s.size() && (s[a] == ' ' || s[a] == '\t')) {
    ++a;
  }
  size_t b = s.size();
  while (b > a && (s[b - 1] == ' ' || s[b - 1] == '\t' || s[b - 1] == '\r')) {
    --b;
  }
  return std::string(s.substr(a, b - a));
}

int leading_spaces(std::string_view s) {
  int n = 0;
  while (n < static_cast<int>(s.size()) && s[static_cast<size_t>(n)] == ' ') {
    ++n;
  }
  return n;
}

bool is_hr(std::string_view line) {
  auto t = trim(line);
  return t == "---" || t == "***" || t == "___" || t == "- - -";
}

bool is_fence(std::string_view line) {
  auto t = trim(line);
  return t.size() >= 3 && t.substr(0, 3) == "```";
}

bool is_md_fence_lang(std::string_view lang) {
  size_t i = 0;
  while (i < lang.size() && (std::isspace(static_cast<unsigned char>(lang[i])) == 0)) {
    ++i;
  }
  std::string word(lang.substr(0, i));
  for (char& c : word) {
    if (c >= 'A' && c <= 'Z') {
      c = static_cast<char>(c - 'A' + 'a');
    }
  }
  return word == "markdown" || word == "md";
}

enum class CodeLanguage { plain, cpp, json, shell, python, yaml, javascript, typescript, rust, go };
enum class CodeToken { plain, keyword, string, comment };
enum class CodeState { normal, block_comment, triple_single, triple_double, template_string };

CodeLanguage code_language(std::string_view label) {
  const auto end = label.find_first_of(" \t");
  auto lang = std::string(label.substr(0, end));
  std::transform(lang.begin(), lang.end(), lang.begin(),
                 [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
  if (lang == "c" || lang == "cpp" || lang == "c++" || lang == "cc" || lang == "cxx" ||
      lang == "h" || lang == "hpp") {
    return CodeLanguage::cpp;
  }
  if (lang == "json") {
    return CodeLanguage::json;
  }
  if (lang == "sh" || lang == "bash" || lang == "zsh" || lang == "shell") {
    return CodeLanguage::shell;
  }
  if (lang == "python" || lang == "py") {
    return CodeLanguage::python;
  }
  if (lang == "yaml" || lang == "yml") {
    return CodeLanguage::yaml;
  }
  if (lang == "js" || lang == "javascript" || lang == "jsx" || lang == "mjs" || lang == "cjs") {
    return CodeLanguage::javascript;
  }
  if (lang == "ts" || lang == "typescript" || lang == "tsx" || lang == "mts" || lang == "cts") {
    return CodeLanguage::typescript;
  }
  if (lang == "rust" || lang == "rs") {
    return CodeLanguage::rust;
  }
  if (lang == "go" || lang == "golang") {
    return CodeLanguage::go;
  }
  return CodeLanguage::plain;
}

bool code_keyword(CodeLanguage lang, std::string_view word) {
  std::string_view words;
  switch (lang) {
  case CodeLanguage::cpp:
    words =
        " alignas alignof auto bool break case catch char class const constexpr consteval "
        "continue decltype default delete do double else enum explicit extern false float for "
        "if inline int long namespace new noexcept nullptr private protected public return short "
        "signed sizeof static struct switch template this throw true try typedef typename union "
        "unsigned using virtual void volatile while ";
    break;
  case CodeLanguage::json:
    words = " true false null ";
    break;
  case CodeLanguage::shell:
    words = " case do done elif else esac fi for function if in select then until while ";
    break;
  case CodeLanguage::python:
    words = " False None True and as assert async await break class continue def del elif "
            "else except finally for from global if import in is lambda nonlocal not or pass "
            "raise return try while with yield ";
    break;
  case CodeLanguage::yaml:
    words = " true false null ";
    break;
  case CodeLanguage::javascript:
    words =
        " async await break case catch class const continue debugger default delete do else "
        "export extends false finally for from function if import in instanceof let new null "
        "of return static super switch this throw true try typeof undefined var void while yield ";
    break;
  case CodeLanguage::typescript:
    words =
        " abstract any as async await boolean break case catch class const continue debugger "
        "declare default delete do else enum export extends false finally for from function "
        "if implements import in infer instanceof interface is keyof let module namespace never "
        "new null number object of override private protected public readonly return satisfies "
        "static string super switch symbol this throw true try type typeof undefined unique "
        "unknown var void while yield ";
    break;
  case CodeLanguage::rust:
    words =
        " as async await break const continue crate dyn else enum extern false fn for if impl "
        "in let loop match mod move mut pub ref return self Self static struct super trait true "
        "type unsafe use where while ";
    break;
  case CodeLanguage::go:
    words =
        " break case chan const continue default defer else fallthrough false for func go "
        "goto if import interface map nil package range return select struct switch true type var ";
    break;
  case CodeLanguage::plain:
    return false;
  }
  auto at = words.find(word);
  while (at != std::string_view::npos) {
    if (at > 0 && words[at - 1] == ' ' && at + word.size() < words.size() &&
        words[at + word.size()] == ' ') {
      return true;
    }
    at = words.find(word, at + 1);
  }
  return false;
}

bool code_identifier(char c) {
  return std::isalnum(static_cast<unsigned char>(c)) != 0 || c == '_';
}

Element render_code_line(std::string_view line, CodeLanguage lang, CodeState& state,
                         const Theme& theme) {
  if (line.empty()) {
    return text(" ") | color(theme.code);
  }
  if (lang == CodeLanguage::plain) {
    return text(std::string(line)) | color(theme.code);
  }
  Elements parts;
  auto add = [&](size_t from, size_t to, CodeToken token) {
    if (from == to) {
      return;
    }
    auto piece = text(std::string(line.substr(from, to - from)));
    const auto tint = token == CodeToken::keyword   ? theme.emphasis
                      : token == CodeToken::string  ? theme.quote
                      : token == CodeToken::comment ? theme.muted
                                                    : theme.code;
    parts.push_back(std::move(piece) | color(tint));
  };
  size_t i = 0;
  size_t plain_start = 0;
  while (i < line.size()) {
    if (state == CodeState::block_comment) {
      const auto end = line.find("*/", i);
      const auto next = end == std::string_view::npos ? line.size() : end + 2;
      add(i, next, CodeToken::comment);
      i = plain_start = next;
      if (end != std::string_view::npos) {
        state = CodeState::normal;
      }
      continue;
    }
    if (state == CodeState::triple_single || state == CodeState::triple_double) {
      const auto marker = state == CodeState::triple_single ? "'''" : "\"\"\"";
      const auto end = line.find(marker, i);
      const auto next = end == std::string_view::npos ? line.size() : end + 3;
      add(plain_start, next, CodeToken::string);
      i = plain_start = next;
      if (end != std::string_view::npos) {
        state = CodeState::normal;
      }
      continue;
    }
    if (state == CodeState::template_string) {
      size_t end = i;
      while (end < line.size()) {
        if (lang != CodeLanguage::go && line[end] == '\\' && end + 1 < line.size()) {
          end += 2;
        } else if (line[end++] == '`') {
          state = CodeState::normal;
          break;
        }
      }
      add(plain_start, end, CodeToken::string);
      i = plain_start = end;
      continue;
    }
    if ((lang == CodeLanguage::cpp || lang == CodeLanguage::javascript ||
         lang == CodeLanguage::typescript || lang == CodeLanguage::rust ||
         lang == CodeLanguage::go) &&
        starts_at(line, i, "/*")) {
      add(plain_start, i, CodeToken::plain);
      state = CodeState::block_comment;
      continue;
    }
    if (((lang == CodeLanguage::cpp || lang == CodeLanguage::javascript ||
          lang == CodeLanguage::typescript || lang == CodeLanguage::rust ||
          lang == CodeLanguage::go) &&
         starts_at(line, i, "//")) ||
        ((lang == CodeLanguage::python || lang == CodeLanguage::shell ||
          lang == CodeLanguage::yaml) &&
         line[i] == '#' &&
         ((lang != CodeLanguage::yaml && lang != CodeLanguage::shell) || i == 0 ||
          std::isspace(static_cast<unsigned char>(line[i - 1])) != 0))) {
      add(plain_start, i, CodeToken::plain);
      add(i, line.size(), CodeToken::comment);
      return hbox(std::move(parts));
    }
    if ((lang == CodeLanguage::javascript || lang == CodeLanguage::typescript ||
         lang == CodeLanguage::go) &&
        line[i] == '`') {
      add(plain_start, i, CodeToken::plain);
      state = CodeState::template_string;
      plain_start = i;
      ++i;
      continue;
    }
    if (line[i] == '"' ||
        (lang != CodeLanguage::json && lang != CodeLanguage::rust && line[i] == '\'')) {
      add(plain_start, i, CodeToken::plain);
      const char quote = line[i];
      if (lang == CodeLanguage::python && i + 3 <= line.size() &&
          line.substr(i, 3) == std::string(3, quote)) {
        state = quote == '\'' ? CodeState::triple_single : CodeState::triple_double;
        plain_start = i;
        i += 3;
        continue;
      }
      const size_t start = i++;
      while (i < line.size()) {
        if (line[i] == '\\' && i + 1 < line.size()) {
          i += 2;
        } else if (line[i++] == quote) {
          break;
        }
      }
      add(start, i, CodeToken::string);
      plain_start = i;
      continue;
    }
    if (code_identifier(line[i]) && (i == 0 || !code_identifier(line[i - 1]))) {
      const size_t start = i;
      while (i < line.size() && code_identifier(line[i])) {
        ++i;
      }
      if (code_keyword(lang, line.substr(start, i - start))) {
        add(plain_start, start, CodeToken::plain);
        add(start, i, CodeToken::keyword);
        plain_start = i;
      }
      continue;
    }
    ++i;
  }
  add(plain_start, line.size(), CodeToken::plain);
  return hbox(std::move(parts));
}

std::string join_lines(const std::vector<std::string>& lines) {
  std::string out;
  for (size_t i = 0; i < lines.size(); ++i) {
    if (i != 0U) {
      out += '\n';
    }
    out += lines[i];
  }
  return out;
}

bool is_ul(std::string_view line, std::string* item) {
  auto t = trim(line);
  if (t.size() >= 2 && (t[0] == '-' || t[0] == '*' || t[0] == '+') && t[1] == ' ') {
    if (item != nullptr) {
      *item = t.substr(2);
    }
    return true;
  }
  return false;
}

bool is_ol(std::string_view line, std::string* item, std::string* num) {
  auto t = trim(line);
  size_t i = 0;
  while (i < t.size() && (std::isdigit(static_cast<unsigned char>(t[i])) != 0)) {
    ++i;
  }
  if (i == 0 || i + 1 >= t.size() || t[i] != '.' || t[i + 1] != ' ') {
    return false;
  }
  if (num != nullptr) {
    *num = std::string(t.substr(0, i + 1));
  }
  if (item != nullptr) {
    *item = std::string(t.substr(i + 2));
  }
  return true;
}

int heading_level(std::string_view line, std::string* title) {
  auto t = trim(line);
  int n = 0;
  while (n < static_cast<int>(t.size()) && t[static_cast<size_t>(n)] == '#') {
    ++n;
  }
  if (n == 0 || n > 6 || n >= static_cast<int>(t.size()) || t[static_cast<size_t>(n)] != ' ') {
    return 0;
  }
  if (title != nullptr) {
    *title = trim(t.substr(static_cast<size_t>(n) + 1));
  }
  return n;
}

bool is_quote(std::string_view line, std::string* body) {
  auto t = trim(line);
  if (t.size() >= 2 && t[0] == '>' && t[1] == ' ') {
    if (body != nullptr) {
      *body = t.substr(2);
    }
    return true;
  }
  if (t == ">") {
    if (body != nullptr) {
      *body = {};
    }
    return true;
  }
  return false;
}

std::vector<std::string> table_cells(std::string_view line) {
  auto t = trim(line);
  if (!t.empty() && t.front() == '|') {
    t.erase(t.begin());
  }
  if (!t.empty() && t.back() == '|') {
    t.pop_back();
  }
  std::vector<std::string> cells;
  std::string cur;
  for (char c : t) {
    if (c == '|') {
      cells.push_back(trim(cur));
      cur.clear();
    } else {
      cur += c;
    }
  }
  cells.push_back(trim(cur));
  return cells;
}

bool is_table_sep(std::string_view line) {
  if (line.find('|') == std::string_view::npos) {
    return false;
  }
  for (const auto& cell : table_cells(line)) {
    if (cell.empty()) {
      return false;
    }
    for (char c : cell) {
      if (c != '-' && c != ':' && c != ' ') {
        return false;
      }
    }
  }
  return true;
}

struct Block {
  enum Kind { para, heading, list, quote, hr, code, table, blank };
  Kind kind = para;
  int level = 0;
  std::string prefix;
  std::string lang;
  std::vector<Span> spans;
  std::vector<std::string> code_lines;
  std::vector<std::vector<std::string>> rows;
};

std::vector<std::string> split_lines(std::string_view text) {
  std::vector<std::string> lines;
  std::string cur;
  auto finish = [&] {
    if (!cur.empty() && cur.back() == '\r') {
      cur.pop_back();
    }
    lines.push_back(std::move(cur));
    cur.clear();
  };
  for (char c : text) {
    if (c == '\n') {
      finish();
    } else {
      cur += c;
    }
  }
  finish();
  return lines;
}

std::vector<Block> parse_blocks(std::string_view text) {
  auto lines = split_lines(text);
  std::vector<Block> out;
  enum { normal, in_code, in_table } state = normal;
  Block code;
  Block table;
  auto flush_code = [&] {
    if (is_md_fence_lang(code.lang)) {
      auto kids = parse_blocks(join_lines(code.code_lines));
      out.insert(out.end(), kids.begin(), kids.end());
    } else {
      out.push_back(std::move(code));
    }
    code = {};
  };
  for (size_t i = 0; i < lines.size(); ++i) {
    const auto& line = lines[i];
    if (state == in_code) {
      if (is_fence(line)) {
        flush_code();
        state = normal;
      } else {
        code.code_lines.push_back(line);
      }
      continue;
    }
    if (state == in_table) {
      if (line.find('|') != std::string::npos) {
        if (table.rows.size() != 1 || !is_table_sep(line)) {
          table.rows.push_back(table_cells(line));
        }
        continue;
      }
      out.push_back(std::move(table));
      table = {};
      state = normal;
    }
    if (is_fence(line)) {
      code = {};
      code.kind = Block::code;
      auto t = trim(line);
      if (t.size() > 3) {
        code.lang = trim(t.substr(3));
      }
      state = in_code;
      continue;
    }
    if (line.find('|') != std::string::npos && i + 1 < lines.size() && is_table_sep(lines[i + 1])) {
      table = {};
      table.kind = Block::table;
      table.rows.push_back(table_cells(line));
      state = in_table;
      continue;
    }
    if (trim(line).empty()) {
      Block b;
      b.kind = Block::blank;
      out.push_back(std::move(b));
      continue;
    }
    if (is_hr(line)) {
      Block b;
      b.kind = Block::hr;
      out.push_back(std::move(b));
      continue;
    }
    std::string title;
    if (int h = heading_level(line, &title)) {
      Block b;
      b.kind = Block::heading;
      b.level = h;
      b.spans = parse_inline(title);
      out.push_back(std::move(b));
      continue;
    }
    std::string item, num;
    if (is_ul(line, &item)) {
      Block b;
      b.kind = Block::list;
      b.level = leading_spaces(line) / 2;
      b.prefix = "• ";
      b.spans = parse_inline(item);
      out.push_back(std::move(b));
      continue;
    }
    if (is_ol(line, &item, &num)) {
      Block b;
      b.kind = Block::list;
      b.level = leading_spaces(line) / 2;
      b.prefix = num + " ";
      b.spans = parse_inline(item);
      out.push_back(std::move(b));
      continue;
    }
    std::string body;
    if (is_quote(line, &body)) {
      Block b;
      b.kind = Block::quote;
      b.spans = parse_inline(body);
      out.push_back(std::move(b));
      continue;
    }
    Block b;
    b.kind = Block::para;
    b.spans = parse_inline(line);
    out.push_back(std::move(b));
  }
  if (state == in_code) {
    flush_code();
  }
  if (state == in_table) {
    out.push_back(std::move(table));
  }
  return out;
}

std::string span_outline(const Span& s) {
  std::string t = s.text;
  if (s.code) {
    t = "[c]" + t + "[/c]";
  }
  if (s.bold) {
    t = "[b]" + t + "[/b]";
  }
  if (s.italic) {
    t = "[i]" + t + "[/i]";
  }
  if (s.strike) {
    t = "[s]" + t + "[/s]";
  }
  if (s.underline) {
    t = "[u]" + t + "[/u]";
  }
  if (s.dim) {
    t = "[d]" + t + "[/d]";
  }
  return t;
}

std::string spans_outline(const std::vector<Span>& spans) {
  std::string out;
  for (const auto& s : spans) {
    out += span_outline(s);
  }
  return out;
}

Element style_span(const Span& s, const Theme& theme) {
  Element e = text(s.text);
  if (s.code) {
    e = e | color(theme.code);
  } else if (s.underline) {
    e = e | underlined | color(theme.accent);
  } else if (s.bold) {
    e = e | bold | color(theme.emphasis);
  } else if (s.italic) {
    e = e | italic | color(theme.italic);
  }
  if (s.strike) {
    e = e | strikethrough;
  }
  if (s.dim && !s.bold && !s.italic && !s.underline && !s.code) {
    e = e | dim;
  }
  return e;
}

Elements flow_spans(const std::vector<Span>& spans, const Theme& theme) {
  Elements flow;
  bool first_token = true;
  auto push = [&](const Span& style, std::string_view text, bool separate) {
    if (text.empty()) {
      return;
    }
    Span piece = style;
    piece.text = text;
    if (separate && !first_token && piece.text.front() != ' ') {
      piece.text.insert(0, " ");
    }
    flow.push_back(style_span(piece, theme));
    first_token = false;
  };
  for (const auto& span : spans) {
    if (span.text.empty()) {
      continue;
    }
    size_t i = 0;
    while (i < span.text.size()) {
      if (span.text[i] == ' ') {
        while (i < span.text.size() && span.text[i] == ' ') {
          ++i;
        }
        continue;
      }
      size_t j = i;
      while (j < span.text.size() && span.text[j] != ' ') {
        ++j;
      }
      // A word wider than the row cannot wrap inside its element, so it is
      // split into chunks the row can break between.
      bool first_chunk = true;
      for (auto chunk : niminal::wrap_chunks(std::string_view(span.text).substr(i, j - i))) {
        push(span, chunk, first_chunk);
        first_chunk = false;
      }
      i = j;
    }
  }
  return flow;
}

Element wrap_spans(const std::vector<Span>& spans, const Theme& theme) {
  auto flow = flow_spans(spans, theme);
  if (flow.empty()) {
    return emptyElement();
  }
  FlexboxConfig cfg;
  cfg.wrap = FlexboxConfig::Wrap::Wrap;
  return flexbox(std::move(flow), cfg);
}

Element render_block(const Block& b, const Theme& theme) {
  switch (b.kind) {
  case Block::blank:
    return text("");
  case Block::hr:
    return separatorLight() | dim;
  case Block::heading: {
    auto title = wrap_spans(b.spans, theme) | bold;
    if (b.level == 1) {
      return vbox({title | color(theme.accent), separatorLight() | dim});
    }
    if (b.level == 2) {
      return title | color(theme.accent);
    }
    if (b.level == 3) {
      return title | color(theme.quote);
    }
    return title | color(theme.quote) | dim;
  }
  case Block::list: {
    std::string pad(static_cast<size_t>(b.level * 2), ' ');
    return hbox({text(pad + b.prefix), wrap_spans(b.spans, theme) | xflex});
  }
  case Block::quote:
    return hbox({text("│ ") | color(theme.quote) | dim, wrap_spans(b.spans, theme) | xflex}) |
           color(theme.quote);
  case Block::code: {
    Elements rows;
    if (!b.lang.empty()) {
      rows.push_back(text(b.lang) | dim);
    }
    const auto lang = code_language(b.lang);
    CodeState state = CodeState::normal;
    for (const auto& line : b.code_lines) {
      rows.push_back(render_code_line(line, lang, state, theme));
    }
    if (rows.empty()) {
      rows.push_back(text(" ") | color(theme.code));
    }
    return hbox({text("│ ") | dim, vbox(std::move(rows)) | xflex});
  }
  case Block::table: {
    std::vector<std::vector<Element>> cells;
    for (size_t r = 0; r < b.rows.size(); ++r) {
      std::vector<Element> row;
      for (const auto& cell : b.rows[r]) {
        auto e = wrap_spans(parse_inline(cell), theme);
        if (r == 0) {
          e = e | bold;
        }
        row.push_back(std::move(e));
      }
      cells.push_back(std::move(row));
    }
    Table table(std::move(cells));
    table.SelectAll().Border(LIGHT);
    table.SelectAll().Separator(LIGHT);
    if (!b.rows.empty()) {
      table.SelectRow(0).Decorate(bold);
    }
    return table.Render() | dim;
  }
  case Block::para:
    return wrap_spans(b.spans, theme);
  }
  return emptyElement();
}

} // namespace

Element render_markdown(std::string_view source, const Theme& theme) {
  Elements rows;
  auto blocks = parse_blocks(source);
  for (size_t i = 0; i < blocks.size();) {
    if (blocks[i].kind == Block::quote) {
      Elements lines;
      while (i < blocks.size() && blocks[i].kind == Block::quote) {
        lines.push_back(wrap_spans(blocks[i].spans, theme));
        ++i;
      }
      rows.push_back(hbox({text("│ ") | color(theme.quote) | dim, vbox(std::move(lines)) | xflex}) |
                     color(theme.quote));
      continue;
    }
    rows.push_back(render_block(blocks[i], theme));
    ++i;
  }
  if (rows.empty()) {
    return emptyElement();
  }
  return vbox(std::move(rows));
}

std::string markdown_outline(std::string_view text) {
  std::ostringstream out;
  for (const auto& b : parse_blocks(text)) {
    switch (b.kind) {
    case Block::blank:
      out << "BLANK\n";
      break;
    case Block::hr:
      out << "HR\n";
      break;
    case Block::heading:
      out << "H" << b.level << " " << spans_outline(b.spans) << '\n';
      break;
    case Block::list:
      out << "LI " << b.prefix << spans_outline(b.spans) << '\n';
      break;
    case Block::quote:
      out << "Q " << spans_outline(b.spans) << '\n';
      break;
    case Block::code:
      out << "CODE " << b.lang << '\n';
      for (const auto& line : b.code_lines) {
        out << "  " << line << '\n';
      }
      break;
    case Block::table:
      out << "TABLE " << b.rows.size() << '\n';
      break;
    case Block::para:
      out << "P " << spans_outline(b.spans) << '\n';
      break;
    }
  }
  return out.str();
}

} // namespace niminal::app
