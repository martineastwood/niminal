#pragma once

#include <atomic>

namespace niminal::app {

// Routes SIGTERM and SIGHUP to the cancel flag, so a run stops where it can and
// the process exits through the normal path that shuts extensions down. The TUI
// screen installs its own handlers for these signals while its loop runs.
void install_shutdown_handlers(std::atomic<bool>* cancel);

// True once a shutdown signal has been received. Safe to read from any thread.
bool shutdown_requested();

} // namespace niminal::app
