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

To build from a source checkout instead, you need CMake 3.22 or later, a C++23
compiler, and OpenSSL 3 development libraries:

```sh
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build
```

The binary is `build/niminal`.

On macOS you may need `brew install cmake llvm openssl@3`. On Debian or Ubuntu:

```sh
sudo apt install cmake g++ libssl-dev zlib1g-dev
```

Run the full validation pipeline before landing changes:

```sh
./dev check
```

## Next steps

- [Quickstart](/guides/quickstart/) for provider setup and your first turn
- [Configuration](/guides/configuration/) for `~/.niminal/config.json`
