#pragma once

#include <filesystem>
#include <mutex>
#include <stdexcept>
#include <string>
#include <string_view>
#include <vector>

namespace niminal::app {

struct WorkspaceError : std::runtime_error {
  using std::runtime_error::runtime_error;
};

class Workspace {
 public:
  explicit Workspace(std::filesystem::path root);

  const std::filesystem::path& root() const { return root_; }
  std::filesystem::path resolve(std::string_view path) const;
  std::string relative(const std::filesystem::path& path) const;
  std::string file_version(const std::filesystem::path& path) const;
  std::vector<std::string> list_files() const;
  void invalidate_listing();

 private:
  std::filesystem::path root_;
  mutable std::mutex files_mu_;
  mutable std::vector<std::string> files_;
  mutable bool files_cached_ = false;
};

}  // namespace niminal::app
