#include "shutdown.hpp"

#include <niminal/types.hpp>

#include <csignal>

namespace niminal::app {
namespace {

volatile std::sig_atomic_t shutdown_flag = 0;
niminal::Cancellation* cancel_flag = nullptr;

void handle_shutdown(int) {
  shutdown_flag = 1;
  if (cancel_flag != nullptr) {
    cancel_flag->notify_from_signal();
  }
}

} // namespace

void install_shutdown_handlers(niminal::Cancellation* cancel) {
  cancel_flag = cancel;
  std::signal(SIGTERM, handle_shutdown);
  std::signal(SIGHUP, handle_shutdown);
}

bool shutdown_requested() {
  return shutdown_flag != 0;
}

} // namespace niminal::app
