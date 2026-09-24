#include "shutdown.hpp"

#include <csignal>

namespace niminal::app {
namespace {

volatile std::sig_atomic_t shutdown_flag = 0;
std::atomic<bool>* cancel_flag = nullptr;

void handle_shutdown(int) {
  shutdown_flag = 1;
  // std::atomic<bool> stores are lock-free, so this is safe in a handler.
  if (cancel_flag != nullptr) {
    cancel_flag->store(true);
  }
}

} // namespace

void install_shutdown_handlers(std::atomic<bool>* cancel) {
  cancel_flag = cancel;
  std::signal(SIGTERM, handle_shutdown);
  std::signal(SIGHUP, handle_shutdown);
}

bool shutdown_requested() {
  return shutdown_flag != 0;
}

} // namespace niminal::app
