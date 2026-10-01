Follow YAGNI principles, and one-liner solutions

Do not shim or maintain legacy behavior, this is greenfield so we can make breaking changes.

# Agent rules

Language: C++23, built with `./dev` (presets and pinned tool versions live there; do not
call `cmake`/`ctest` by hand). Aim: RAII + explicit ownership + clang-tidy + sanitizers +
warnings-as-errors.

## Repository map

- `src/agent/` → `niminal_ai`, the provider-agnostic agent loop. Public headers live in
  `include/niminal/` (`niminal/ai.hpp` is the embedding API). Keep it free of app and TUI
  dependencies.
- `src/app/` → `niminal_app`, the TUI, sessions, config, providers, tools, extensions.
  Headers sit beside their `.cpp` files and `src/app` is on the include path, so write
  `#include "session.hpp"`, not a path prefix.
- `src/app/main.cpp` → `niminal_cli`, output name `niminal` (`build/dev/niminal`).
- `tests/<name>_test.cpp` is CTest test `<name>`. New files are not auto-discovered: add
  `niminal_add_test(<name> niminal_app)` (or `niminal::ai`) to `CMakeLists.txt`. Tests that
  spawn the binary pass `$<TARGET_FILE:niminal_cli>` and need `add_dependencies`.
- `docs/` is the Astro/Starlight site for niminal.dev. `.github/workflows/ci.yml` is the
  authority on what CI runs.

## Setup

- Configure fails unless CAIL 0.4.0 is installed. `./dev setup` uses a CAIL checkout at
  `../cail`, builds and installs it, then configures `build/dev`. macOS gets OpenSSL from
  Homebrew.
- `./dev` looks for `clang-format-20`/`clang-tidy-20`, then Homebrew `llvm@20` paths. An
  unversioned `clang-format` on PATH is ignored, so install LLVM 20 (`brew install llvm@20`).
- Presets: `build/dev` (Debug), `build/release`, `build/asan` (Debug + `NIMINAL_SANITIZERS`).
  `BUILD_DIR` and `ASAN_DIR` override the directories.
- CMake patches the fetched FTXUI sources in place under `build/*/_deps` at configure time.
  Bumping that `GIT_TAG` only works while the patches still match; configure fails loudly if
  they do not.

## Validation

Configure once per build directory, then build the affected target and run its test suite:

```sh
./dev configure
./dev test --suite agent
```

`--suite NAME` builds `niminal_NAME_test` and runs the CTest test `NAME`. List every suite
with `./dev test -N`. For documentation-only changes skip the C++ build and run
`cd docs && npm run build`.

Before pushing, and for release readiness or major build or toolchain changes, run the full
pipeline in CI order — format, Release configure and build, clang-tidy, unit tests,
ASan/UBSan:

```sh
./dev check
./dev check --no-tidy --no-sanitizer   # the main job in ci.yml
```

`./dev format --fix` rewrites formatting, `./dev tidy --fix` applies clang-tidy fixes,
`./dev sanitizer` runs only the ASan/UBSan build and tests, and `./dev tidy --changed`
tidies changed `src/`+`include/` `.cpp` files only (needs ripgrep on PATH). The full
pipeline is intentionally not part of the routine edit loop.

Do not suppress diagnostics unless you can explain why the diagnostic does not
represent a real defect.

## Code conventions

- clang-format: LLVM base, 2-space indent, 100 columns, braces attach.
- Warnings are errors, including `-Wconversion`, `-Wsign-conversion`, `-Wshadow`, and
  `-Wold-style-cast`. Write conversion-safe code instead of casting around them.
- unique_ptr = exclusive ownership; shared_ptr = shared lifetime, avoid unless genuinely
  required.
- T& = non-null borrow; const T& = immutable borrow; T* = optional non-owning pointer only.
  Raw owning pointers are forbidden.
- span<T> for borrowed contiguous ranges; do not pass pointer + size separately.
- Recoverable domain failures use expected<T, Error>; programming errors use
  assertions/invariants.
- reinterpret_cast, placement new, custom allocators, pointer arithmetic, manual lifetime
  management and unchecked casts require justification and dedicated tests.

## Tests

Bug fixes require regression tests. New public behavior requires tests.

## Background

`README.md` (build from source), `docs/src/content/docs/reference/architecture.md`, and
`docs/src/content/docs/guides/instructions.md` cover the parts this file does not.
