#include "images.hpp"

#include <niminal/text.hpp>

#include <algorithm>
#include <cctype>
#include <fstream>
#include <iterator>
#include <stdexcept>

namespace niminal::app {
namespace {

constexpr size_t kMaxImageBytes = 10 * 1024 * 1024;

std::string mime_type(std::string_view bytes) {
  if (bytes.size() >= 8 && bytes.substr(0, 8) == "\x89PNG\r\n\x1a\n") {
    return "image/png";
  }
  if (bytes.size() >= 3 && static_cast<unsigned char>(bytes[0]) == 0xff &&
      static_cast<unsigned char>(bytes[1]) == 0xd8 &&
      static_cast<unsigned char>(bytes[2]) == 0xff) {
    return "image/jpeg";
  }
  if (bytes.size() >= 12 && bytes.substr(0, 4) == "RIFF" && bytes.substr(8, 4) == "WEBP") {
    return "image/webp";
  }
  return {};
}

std::string image_filename(std::string_view stem, std::string_view mime) {
  if (mime == "image/jpeg") {
    return std::string(stem) + ".jpg";
  }
  if (mime == "image/webp") {
    return std::string(stem) + ".webp";
  }
  return std::string(stem) + ".png";
}

} // namespace

bool image_path(std::string_view path) {
  auto dot = path.find_last_of('.');
  if (dot == std::string_view::npos) {
    return false;
  }
  std::string ext(path.substr(dot));
  std::transform(ext.begin(), ext.end(), ext.begin(),
                 [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
  return ext == ".png" || ext == ".jpg" || ext == ".jpeg" || ext == ".webp";
}

niminal::json image_part(std::string_view bytes, std::string_view name) {
  if (bytes.size() > kMaxImageBytes) {
    throw niminal::Error("image exceeds 10 MiB: " + std::string(name));
  }
  auto mime = mime_type(bytes);
  if (mime.empty()) {
    throw niminal::Error("unsupported or invalid image: " + std::string(name));
  }
  return niminal::json{{"type", "image"},
                       {"name", name},
                       {"mime_type", mime},
                       {"data", niminal::base64_encode(bytes)}};
}

niminal::json clipboard_image_part(std::string_view bytes) {
  const auto mime = mime_type(bytes);
  if (mime.empty()) {
    throw niminal::Error("unsupported or invalid clipboard image");
  }
  return image_part(bytes, image_filename("clipboard", mime));
}

niminal::json read_image(const std::filesystem::path& path) {
  std::error_code ec;
  const auto size = std::filesystem::file_size(path, ec);
  if (ec) {
    throw niminal::Error("cannot read image: " + path.string());
  }
  if (size > kMaxImageBytes) {
    throw niminal::Error("image exceeds 10 MiB: " + path.string());
  }
  std::ifstream in(path, std::ios::binary);
  if (!in) {
    throw niminal::Error("cannot read image: " + path.string());
  }
  std::string bytes((std::istreambuf_iterator<char>(in)), {});
  return image_part(bytes, path.filename().string());
}

} // namespace niminal::app
