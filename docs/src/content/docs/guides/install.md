---
title: Install
description: Install the niminal release binary on macOS or Linux, or build from source.
---

You can install niminal with a one-line script that downloads the latest release
tarball for your platform. No compiler or source checkout is required.

After install, continue with the [Quickstart](/guides/quickstart/) to set a
provider key and run your first turn.

## macOS and Linux

```sh
curl -fsSL https://niminal.dev/install.sh | sh
```

The installer puts the `niminal` binary in `~/.local/bin`. Add that directory
to your `PATH` if it is not already there.

Pin a release with `NIMINAL_VERSION=v0.1.0` before the curl command, or override
the install location with `NIMINAL_INSTALL_DIR`.

Published platforms today:

| Platform | Architecture |
| --- | --- |
| Linux | x86_64 |
| macOS | arm64, x86_64 |

Linux arm64 binaries are not published yet.

## Build from source

To build from a source checkout, you need CMake 3.22 or later, Ninja, a C++23
compiler, OpenSSL 3 development libraries, and zlib development libraries:

```sh
cmake --preset dev
cmake --build --preset dev
```

The binary is `build/dev/niminal`.

On macOS, install the build tools and libraries with Homebrew:

```sh
brew install cmake ninja llvm openssl@3
```

On Debian or Ubuntu:

```sh
sudo apt install cmake ninja-build g++ libssl-dev zlib1g-dev
```

To run `./dev check`, also install `clang-format`, `clang-tidy`, and
`run-clang-tidy`. On macOS, `llvm` provides these tools. On Debian or Ubuntu:

```sh
sudo apt install clang-format clang-tidy
```

For quick feedback, build and test only the affected suite. This example builds
and runs the agent tests:

```sh
./dev configure
./dev build --target niminal_agent_test
./dev test -R '^agent$'
```

Run the full validation pipeline before completing a change:

```sh
./dev check
```

It checks formatting, builds the project, runs clang-tidy and all unit tests,
then builds and tests with ASan/UBSan.

## Next steps

- [Quickstart](/guides/quickstart/) for provider setup and your first turn
- [Configuration](/guides/configuration/) for `~/.niminal/config.json`
