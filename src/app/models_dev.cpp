#include "models_dev.hpp"

#include "config.hpp"
#include <niminal/text.hpp>

#include <niminal/http.hpp>
#include <nlohmann/json.hpp>

#include <algorithm>
#include <chrono>
#include <cstdio>
#include <fstream>
#include <mutex>
#include <system_error>

namespace niminal::app {
namespace {

using json = nlohmann::json;
namespace fs = std::filesystem;

constexpr const char* kUrl = "https://models.dev/api.json";

std::mutex g_mu;
std::vector<CatalogModel> g_models;
fs::path g_path;
bool g_loaded = false;

fs::path default_path() {
  try {
    return config_path().parent_path() / "models-dev.json";
  } catch (...) {
    return {};
  }
}

std::vector<CatalogModel> parse_catalog(const json& doc) {
  std::vector<CatalogModel> out;
  if (!doc.is_object()) {
    return out;
  }
  for (auto& [provider, block] : doc.items()) {
    if (!block.is_object() || !block.contains("models") || !block["models"].is_object()) {
      continue;
    }
    for (auto& [id, model] : block["models"].items()) {
      if (id.empty()) {
        continue;
      }
      CatalogModel row;
      row.provider = provider;
      row.id = id;
      if (!model.is_object()) {
        out.push_back(std::move(row));
        continue;
      }
      if (model.contains("limit") && model["limit"].is_object()) {
        row.context = model["limit"].value("context", 0);
      }
      if (model.contains("cost") && model["cost"].is_object()) {
        const auto& cost = model["cost"];
        auto price = [&](const char* key) {
          auto it = cost.find(key);
          return it != cost.end() && it->is_number() ? it->get<double>() : 0.0;
        };
        row.cost.input = price("input");
        row.cost.output = price("output");
        row.cost.cache_read = price("cache_read");
        row.cost.cache_write = price("cache_write");
        row.cost.known = row.cost.input > 0 || row.cost.output > 0 || row.cost.cache_read > 0 ||
                         row.cost.cache_write > 0;
      }
      if (model.contains("reasoning") && model["reasoning"].is_boolean()) {
        row.reasoning = model["reasoning"].get<bool>();
      }
      if (model.contains("reasoning_options") && model["reasoning_options"].is_array()) {
        for (const auto& option : model["reasoning_options"]) {
          if (!option.is_object()) {
            continue;
          }
          auto type = option.value("type", "");
          if (type == "toggle") {
            row.toggle = true;
          } else if (type == "budget_tokens") {
            row.budget_tokens = true;
          } else if (type == "effort" && option.contains("values") && option["values"].is_array()) {
            for (const auto& value : option["values"]) {
              if (!value.is_string()) {
                continue;
              }
              auto effort = niminal::lower_copy(value.get<std::string>());
              if (effort.empty()) {
                continue;
              }
              if (std::find(row.efforts.begin(), row.efforts.end(), effort) == row.efforts.end()) {
                row.efforts.push_back(effort);
              }
            }
          }
        }
      }
      if (row.toggle || row.budget_tokens || !row.efforts.empty()) {
        row.reasoning = true;
      }
      out.push_back(std::move(row));
    }
  }
  return out;
}

bool read_file(const fs::path& path, std::vector<CatalogModel>& out) {
  std::error_code ec;
  if (path.empty() || !fs::exists(path, ec)) {
    return false;
  }
  std::ifstream in(path);
  if (!in) {
    return false;
  }
  try {
    json doc = json::parse(in);
    out = parse_catalog(doc);
    return true;
  } catch (...) {
    return false;
  }
}

fs::path path_locked() {
  return g_path.empty() ? default_path() : g_path;
}

void ensure_locked() {
  if (g_loaded) {
    return;
  }
  std::vector<CatalogModel> models;
  read_file(path_locked(), models);
  g_models = std::move(models);
  g_loaded = true;
}

} // namespace

std::filesystem::path catalog_cache_path() {
  std::lock_guard<std::mutex> lock(g_mu);
  return path_locked();
}

void set_catalog_cache_path(const fs::path& path) {
  std::lock_guard<std::mutex> lock(g_mu);
  g_path = path;
  g_loaded = false;
  g_models.clear();
}

std::string catalog_name(std::string_view provider) {
  auto p = niminal::lower_copy(std::string(provider));
  if (p == "opencode") {
    return "opencode-go";
  }
  if (p == "opencodezen") {
    return "opencode";
  }
  return p;
}

bool load_catalog() {
  std::lock_guard<std::mutex> lock(g_mu);
  g_loaded = false;
  ensure_locked();
  return !g_models.empty();
}

bool catalog_stale(int max_age_seconds) {
  auto path = catalog_cache_path();
  std::error_code ec;
  if (path.empty() || !fs::exists(path, ec)) {
    return true;
  }
  auto mtime = fs::last_write_time(path, ec);
  if (ec) {
    return true;
  }
  auto now = fs::file_time_type::clock::now();
  auto age = std::chrono::duration_cast<std::chrono::seconds>(now - mtime);
  return age.count() >= max_age_seconds;
}

bool refresh_catalog() {
  niminal::HttpClient http;
  auto res = http.get(kUrl, 20);
  if (!res || res->status >= 400 || res->body.empty()) {
    return false;
  }
  json doc;
  try {
    doc = json::parse(res->body);
  } catch (...) {
    return false;
  }
  auto models = parse_catalog(doc);
  if (models.empty()) {
    return false;
  }
  auto path = catalog_cache_path();
  if (path.empty()) {
    return false;
  }
  std::error_code ec;
  fs::create_directories(path.parent_path(), ec);
  auto tmp = path;
  tmp += ".tmp";
  {
    std::ofstream out(tmp, std::ios::binary | std::ios::trunc);
    if (!out) {
      return false;
    }
    out << res->body;
  }
  fs::rename(tmp, path, ec);
  if (ec) {
    fs::remove(tmp, ec);
    return false;
  }
  std::lock_guard<std::mutex> lock(g_mu);
  g_models = std::move(models);
  g_loaded = true;
  return true;
}

std::string format_context_k(int tokens) {
  if (tokens <= 0) {
    return {};
  }
  if (tokens >= 1000) {
    return std::to_string(tokens / 1000) + "k";
  }
  return std::to_string(tokens);
}

std::vector<CatalogModel> search_catalog(std::string_view provider, std::string_view query, int cap,
                                         const std::vector<std::string>& skip) {
  if (cap <= 0) {
    return {};
  }
  auto want = catalog_name(provider);
  auto q = niminal::lower_copy(std::string(query));
  std::vector<std::string> skip_l;
  skip_l.reserve(skip.size());
  for (const auto& s : skip) {
    skip_l.push_back(niminal::lower_copy(s));
  }
  auto skipped = [&](const std::string& id) {
    auto l = niminal::lower_copy(id);
    return std::find(skip_l.begin(), skip_l.end(), l) != skip_l.end();
  };
  std::lock_guard<std::mutex> lock(g_mu);
  ensure_locked();
  std::vector<CatalogModel> out;
  for (const auto& row : g_models) {
    if (row.provider != want) {
      continue;
    }
    if (skipped(row.id)) {
      continue;
    }
    if (!q.empty() && niminal::lower_copy(row.id).find(q) == std::string::npos) {
      continue;
    }
    out.push_back(row);
  }
  std::sort(out.begin(), out.end(),
            [](const CatalogModel& a, const CatalogModel& b) { return a.id < b.id; });
  if (static_cast<int>(out.size()) > cap) {
    out.resize(static_cast<size_t>(cap));
  }
  return out;
}

ReasoningCaps lookup_reasoning_caps(std::string_view provider, std::string_view model) {
  ReasoningCaps caps;
  if (provider.empty() || model.empty()) {
    return caps;
  }
  auto want_p = catalog_name(provider);
  auto want_m = niminal::lower_copy(std::string(model));
  std::lock_guard<std::mutex> lock(g_mu);
  ensure_locked();
  for (const auto& row : g_models) {
    if (row.provider != want_p || niminal::lower_copy(row.id) != want_m) {
      continue;
    }
    caps.known = true;
    caps.reasoning = row.reasoning;
    caps.toggle = row.toggle;
    caps.budget_tokens = row.budget_tokens;
    caps.efforts = row.efforts;
    return caps;
  }
  return caps;
}

ModelCost lookup_model_cost(std::string_view provider, std::string_view model) {
  if (provider.empty() || model.empty()) {
    return {};
  }
  auto want_p = catalog_name(provider);
  auto want_m = niminal::lower_copy(std::string(model));
  std::lock_guard<std::mutex> lock(g_mu);
  ensure_locked();
  for (const auto& row : g_models) {
    if (row.provider == want_p && niminal::lower_copy(row.id) == want_m) {
      return row.cost;
    }
  }
  return {};
}

double usage_cost_usd(const niminal::Usage& usage, const ModelCost& cost) {
  // OpenAI-style `prompt_tokens` already include cached reads, Anthropic-style `input_tokens`
  // exclude cache reads and writes. Treat input as inclusive when it can cover the cached reads
  // (the same heuristic `format_usage_line` uses for its cache percentage).
  double full_input = usage.input_tokens -
                      (usage.cache_read_tokens <= usage.input_tokens ? usage.cache_read_tokens : 0);
  return (full_input * cost.input + usage.cache_read_tokens * cost.cache_read +
          usage.cache_write_tokens * cost.cache_write + usage.output_tokens * cost.output) /
         1'000'000.0;
}

std::string format_cost_usd(double usd) {
  char buf[32];
  std::snprintf(buf, sizeof(buf), usd < 0.01 ? "$%.4f" : "$%.2f", usd);
  return buf;
}

} // namespace niminal::app
