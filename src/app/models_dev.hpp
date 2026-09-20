#pragma once

#include <filesystem>
#include <string>
#include <string_view>
#include <vector>

namespace niminal::app {

struct CatalogModel {
  std::string provider;
  std::string id;
  int context = 0;
  bool reasoning = false;
  bool toggle = false;
  bool budget_tokens = false;
  std::vector<std::string> efforts;
};

struct ReasoningCaps {
  bool known = false;
  bool reasoning = false;
  bool toggle = false;
  bool budget_tokens = false;
  std::vector<std::string> efforts;
};

std::filesystem::path catalog_cache_path();
void set_catalog_cache_path(const std::filesystem::path& path);
std::string catalog_name(std::string_view provider);
bool load_catalog();
bool catalog_stale(int max_age_seconds = 24 * 60 * 60);
bool refresh_catalog();
std::string format_context_k(int tokens);
std::vector<CatalogModel> search_catalog(std::string_view provider,
                                         std::string_view query, int cap,
                                         const std::vector<std::string>& skip = {});
ReasoningCaps lookup_reasoning_caps(std::string_view provider,
                                    std::string_view model);

}  // namespace niminal::app
