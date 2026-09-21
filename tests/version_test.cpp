#include <niminal/version.hpp>

#include <array>
#include <cstdio>
#include <filesystem>
#include <iostream>
#include <string>
#include <unistd.h>

namespace {

struct Run {
  int status = -1;
  std::string output;
};

Run run(const std::string& command) {
  Run result;
  FILE* pipe = popen(command.c_str(), "r");
  if (pipe == nullptr) {
    return result;
  }
  std::array<char, 256> buffer{};
  while (std::fgets(buffer.data(), static_cast<int>(buffer.size()), pipe) != nullptr) {
    result.output += buffer.data();
  }
  result.status = pclose(pipe);
  return result;
}

int fail(const std::string& message) {
  std::cerr << message << '\n';
  return 1;
}

} // namespace

int main(int argc, char** argv) {
  if (argc < 2) {
    return fail("usage: version_test <path-to-niminal>");
  }
  const std::string binary = argv[1];
  const auto home =
      std::filesystem::temp_directory_path() / ("niminal-version-" + std::to_string(getpid()));
  std::filesystem::create_directories(home);
  const std::string prefix = "HOME=" + home.string() + " " + binary;

  const std::string expected = niminal::version_string() + "\n";
  for (const char* flag : {"--version", "-v"}) {
    auto result = run(prefix + " " + flag);
    if (result.status != 0 || result.output != expected) {
      std::filesystem::remove_all(home);
      return fail(std::string(flag) + " printed " + result.output);
    }
  }

  auto help = run(prefix + " --help");
  std::filesystem::remove_all(home);
  if (help.status != 0 || help.output.find("--version") == std::string::npos) {
    return fail("--help does not mention --version");
  }
  return 0;
}
