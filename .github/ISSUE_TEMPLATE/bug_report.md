---
name: Bug report
about: Something broke, sounded wrong, or crashed
---

**What happened, and what you expected**

**Version** (`niminal --version`, and the app if Tracker or Stage) and macOS version:

**A minimal `.nml` file or the steps that show it** (what you ran: `render`, `server`, the Tracker, ...):

**Logs**, if it crashed or the server stopped (all in `~/.niminal/`):
- `crash.log`: a panic, with a backtrace
- `server.log` (and `server.log.old` for the run before): what the server printed
- `sessions/`: the log of each session
