#pragma once

#include <niminal/types.hpp>

#include <filesystem>
#include <string>
#include <string_view>
#include <vector>

namespace niminal::app {

// Prices are USD per million tokens, as published by models.dev per model.
struct ModelCost {
  bool known = false;
  double input = 0;
  double output = 0;
  double cache_read = 0;
  double cache_write = 0;
};

struct CatalogModel {
  std::string provider;
  std::string id;
  std::string sdk;
  int context = 0;
  bool reasoning = false;
  bool toggle = false;
  bool budget_tokens = false;
  std::vector<std::string> efforts;
  ModelCost cost;
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
std::vector<CatalogModel> search_catalog(std::string_view provider, std::string_view query, int cap,
                                         const std::vector<std::string>& skip = {});
ReasoningCaps lookup_reasoning_caps(std::string_view provider, std::string_view model);
ModelCost lookup_model_cost(std::string_view provider, std::string_view model);
std::string lookup_model_sdk(std::string_view provider, std::string_view model);
double usage_cost_usd(const niminal::Usage& usage, const ModelCost& cost);
std::string format_cost_usd(double usd);

} // namespace niminal::app
