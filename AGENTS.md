Follow YAGNI principles, and one-liner solutions

Do not shim or maintain legacy behavior, this is greenfield so we can make breaking changes.

# Agent rules

Language: C++23.

Toolchain: clang-format 20 and clang-tidy 20, matching CI. Other versions drift on formatting and can reject the C++23 standard library headers.

We are aiming for: C++23 + RAII + explicit ownership + clang-tidy + sanitizers + warnings-as-errors + deterministic build/test commands

## Validation

Use focused checks while iterating. Configure once per build directory, then
build the affected target and run its test suite:

```sh
./dev configure
./dev test --suite agent
```

For documentation-only changes, skip the C++ build and tests. When a test suite
name is unclear, list them with `./dev test -N`.

## Ownership

unique_ptr = exclusive ownership
shared_ptr = shared lifetime; avoid unless genuinely required
T& = non-null borrow
const T& = immutable borrow
T* = optional non-owning pointer only

Raw owning pointers are forbidden.

## Containers

Use span<T> for borrowed contiguous ranges.
Do not pass pointer + size separately.

## Errors

Recoverable domain failures use expected<T, Error>.
Programming errors use assertions/invariants.

## Unsafe operations

reinterpret_cast, placement new, custom allocators, pointer arithmetic,
manual lifetime management and unchecked casts require justification
and dedicated tests.

## Checks

Run `./dev check` for release readiness, major build or toolchain changes, or
when a focused check cannot cover the change. It runs formatting, a Release
build, clang-tidy across project sources, all unit tests, and ASan/UBSan tests.
This full pipeline is intentionally not part of the routine edit loop.

Do not suppress diagnostics unless you can explain why the diagnostic does not
represent a real defect.

## Tests

Bug fixes require regression tests.
New public behavior requires tests.

## Full validation

`./dev check` runs the full validation pipeline, in order:

1. `clang-format` check
2. CMake configure (`build/release/`)
3. Release build with warnings as errors
4. `clang-tidy` on `src/` and `include/`
5. Unit tests (`ctest`)
6. ASan/UBSan configure, build, and test (`build/asan/`)

Use the focused suite command above for quick iteration.

Other commands:

```sh
./dev configure          # configure the development build
./dev test --suite agent  # build and run one CTest suite by name
./dev format --fix   # rewrite formatting
./dev tidy --fix     # apply clang-tidy fixes
./dev sanitizer      # ASan/UBSan build and test only
```

Run `./dev test -N` to list the available suites.
