#pragma once

#include <filesystem>
#include <optional>
#include <string>
#include <vector>

namespace niminal::app {

struct PromptTemplate {
  std::string name;
  std::string description;
  std::string body;
  std::filesystem::path path;
};

std::vector<PromptTemplate> discover_prompts(const std::filesystem::path& workspace);
std::optional<PromptTemplate> load_prompt(const std::filesystem::path& workspace,
                                          const std::string& name);
std::string expand_prompt(const std::filesystem::path& workspace, const std::string& name,
                          const std::string& arguments);

} // namespace niminal::app
