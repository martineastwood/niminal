#include "mentions.hpp"

#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>

namespace fs = std::filesystem;
using namespace niminal::app;

int main() {
  auto root = fs::temp_directory_path() / "niminal-mentions-test";
  fs::remove_all(root);
  fs::create_directories(root / "src");
  fs::create_directories(root / "node_modules/pkg");
  fs::create_directories(root / "generated");
  std::ofstream(root / "src/agent.cpp") << "int agent = 1;\n";
  std::ofstream(root / "README.md") << "hello\n";
  std::ofstream(root / "node_modules/pkg/index.js") << "ignored\n";
  std::ofstream(root / "generated/ignored.cpp") << "ignored\n";
  std::ofstream(root / ".gitignore") << "generated/\n";
  if (std::system(("git -C " + root.string() + " init -q").c_str()) != 0) {
    return 1;
  }

  Workspace workspace(root);
  auto mention = file_mention_at("fix @agen please", 9);
  if (!mention || mention->query != "agen") {
    return 1;
  }
  if (file_mention_at("user@example.com", 12)) {
    return 2;
  }

  auto files = suggest_mentioned_files(workspace, "agen");
  if (files != std::vector<std::string>{"src/agent.cpp"}) {
    return 3;
  }
  for (const auto& file : suggest_mentioned_files(workspace, "")) {
    if (file.find("node_modules") != std::string::npos ||
        file.find("generated") != std::string::npos) {
      return 4;
    }
  }

  auto applied = apply_file_mention("fix @agen please", 9, "src/agent.cpp");
  if (applied != "fix @src/agent.cpp  please") {
    return 5;
  }

  auto expanded = expand_file_mentions(workspace, "review @src/agent.cpp");
  if (expanded.find("<file path=\"src/agent.cpp\">") == std::string::npos ||
      expanded.find("int agent = 1;") == std::string::npos) {
    return 6;
  }
  if (expand_file_mentions(workspace, "mail user@example.com") != "mail user@example.com") {
    return 7;
  }

  {
    std::ofstream image(root / "screen shot.png", std::ios::binary);
    image.write("\x89PNG\r\n\x1a\n", 8);
  }
  auto mentioned = prepare_user_input(workspace, "inspect @screen shot.png");
  if (!mentioned.images.empty()) {
    return 8; // Paths with spaces need quoting or a drop.
  }
  mentioned = prepare_user_input(workspace, "inspect @README.md");
  if (mentioned.text.find("hello") == std::string::npos || !mentioned.images.empty()) {
    return 9;
  }
  fs::rename(root / "screen shot.png", root / "screen.png");
  mentioned = prepare_user_input(workspace, "inspect @screen.png");
  if (mentioned.images.size() != 1 || mentioned.images[0].value("mime_type", "") != "image/png") {
    return 10;
  }
  auto dropped = prepare_user_input(workspace, "screen.png");
  if (!dropped.text.empty() || dropped.images.size() != 1) {
    return 11;
  }
  dropped = prepare_user_input(workspace, std::string("file://") + (root / "screen.png").string());
  if (!dropped.text.empty() || dropped.images.size() != 1) {
    return 13;
  }
  {
    std::ofstream jpeg(root / "photo.jpg", std::ios::binary);
    jpeg.write("\xff\xd8\xff", 3);
    std::ofstream webp(root / "shot.webp", std::ios::binary);
    webp.write("RIFF\0\0\0\0WEBP", 12);
  }
  dropped = prepare_user_input(workspace, "photo.jpg shot.webp");
  if (dropped.images.size() != 2 || dropped.images[0].value("mime_type", "") != "image/jpeg" ||
      dropped.images[1].value("mime_type", "") != "image/webp") {
    return 14;
  }
  {
    std::ofstream image(root / "bad.png", std::ios::binary);
    image << "not an image";
  }
  try {
    (void)prepare_user_input(workspace, "@bad.png");
    return 12;
  } catch (const niminal::Error&) {
  }

  fs::remove_all(root);
  return 0;
}
