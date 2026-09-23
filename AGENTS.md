Follow YAGNI principles, and one-liner solutions

Do not shim or maintain legacy behavior, this is greenfield so we can make breaking changes.

# Agent rules

Language: C++23.

We are aiming for: C++23 + RAII + explicit ownership + clang-tidy + sanitizers + warnings-as-errors + deterministic build/test commands

## Validation

Configure once with `./dev configure`. For each change, build and run the
smallest affected test suite. For example:

```sh
./dev build --target niminal_agent_test
./dev test -R '^agent$'
```

Run `./dev check` before completing a change. It runs formatting, the Release
build, clang-tidy across project sources, all unit tests, and ASan/UBSan tests.
Use focused build and test commands while iterating instead of running the full
pipeline after every edit.

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

Do not complete a change until `./dev check` succeeds.

Do not suppress diagnostics unless you can explain why the diagnostic
does not represent a real defect.

## Tests

Bug fixes require regression tests.
New public behavior requires tests.

## Dev check

`./dev check` runs, in order:

1. `clang-format` check
2. CMake configure (`build/dev/`)
3. Release build with warnings as errors
4. `clang-tidy` on `src/` and `include/`
5. Unit tests (`ctest`)
6. ASan/UBSan configure, build, and test (`build/asan/`)

Other commands:

```sh
./dev configure          # configure the development build
./dev build --target niminal_agent_test
./dev test -R '^agent$'  # run one CTest suite by name
./dev format --fix   # rewrite formatting
./dev tidy --fix     # apply clang-tidy fixes
./dev sanitizer      # ASan/UBSan build and test only
```

The focused test name is a CTest regular expression. Run `./dev test -N` to
list the available suites.
