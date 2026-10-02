---
title: Install
description: Install the niminal release binary on macOS 15+, Linux, or Windows via WSL, or build from source.
---

You can install niminal with a one-line script that downloads the latest release
tarball for your platform. No compiler or source checkout is required.

After install, continue with the [Quickstart](/guides/quickstart/) to set a
provider key and run your first turn.

## macOS 15+ and Linux

```sh
curl -fsSL https://niminal.dev/install.sh | sh
```

The installer puts the `niminal` binary in `~/.local/bin`. Add that directory
to your `PATH` if it is not already there.

To install a specific release, set `NIMINAL_VERSION` for the installer shell:

```sh
curl -fsSL https://niminal.dev/install.sh | NIMINAL_VERSION=v0.3.1 sh
```

Set `NIMINAL_INSTALL_DIR` before `sh` in the pipeline to choose a different
install location:

```sh
curl -fsSL https://niminal.dev/install.sh | NIMINAL_VERSION=v0.3.1 NIMINAL_INSTALL_DIR="$HOME/bin" sh
```

Release platforms:

| Platform | Architecture |
| --- | --- |
| Linux (glibc 2.27+) | x86_64, arm64 |
| macOS 15+ | arm64, x86_64 |

You can use the Linux binary on Ubuntu 18.04+, Debian 10+, Rocky/AlmaLinux 8+,
and Amazon Linux 2023. Alpine Linux needs a glibc environment.

## HTTPS certificates

On macOS, niminal uses Apple's system root certificates. You do not need Homebrew
for the release binary. On Linux, install your distribution's `ca-certificates`
package, especially in minimal containers:

```sh
# Ubuntu or Debian
sudo apt-get install ca-certificates

# Rocky, AlmaLinux, or Fedora
sudo dnf install ca-certificates
```

If your network requires a custom certificate bundle, you can set its path:

```sh
SSL_CERT_FILE=/path/to/company-ca-bundle.pem niminal
```

`SSL_CERT_FILE` and `SSL_CERT_DIR` replace the default trust roots. Your bundle
should include every CA you need. niminal still verifies certificates and server
hostnames. Custom macOS Keychain trust settings are not imported automatically;
use a custom bundle for those certificates.

Linux release binaries do not automatically load your distribution's OpenSSL
crypto policies. If you need a custom OpenSSL configuration, set `OPENSSL_CONF`
to a file compatible with the bundled OpenSSL 3.5.

## Windows via WSL

There is no native Windows build. niminal runs inside WSL2, which uses the Linux
binary unchanged.

Use a distro with glibc 2.27 or newer, such as Ubuntu 22.04 or 24.04.

```powershell
wsl --install -d Ubuntu-24.04
```

Then install niminal inside the distro, exactly as on Linux:

```sh
curl -fsSL https://niminal.dev/install.sh | sh
```

Keep the project in the Linux filesystem, such as `~/projects/my-app`. Working
under `/mnt/c` is slower.

## Build from source

To build from a source checkout, you need CMake 3.22 or later, Ninja, a C++23
compiler, OpenSSL 3 development libraries, and CAIL v0.4.1. CAIL requires
CMake 3.31 or later and network access while its dependencies are downloaded.

Install CAIL next to your Niminal checkout:

```sh
git clone --branch v0.4.1 --depth 1 https://github.com/martineastwood/cail.git ../cail
cmake -S ../cail -B ../cail/build -DCAIL_BUILD_EXAMPLES=OFF
cmake --build ../cail/build
cmake --install ../cail/build --prefix ../cail/build/install
export CMAKE_PREFIX_PATH="$PWD/../cail/build/install"
```

Then configure and build Niminal:

```sh
cmake --preset dev
cmake --build --preset dev
```

The binary is `build/dev/niminal`.

On macOS, install the build tools and libraries with Homebrew:

```sh
brew install cmake ninja llvm@20 openssl@3
export PATH="/opt/homebrew/opt/llvm@20/bin:$PATH"
```

On Debian or Ubuntu:

```sh
sudo apt install cmake ninja-build g++ libssl-dev
```

For full release validation with `./dev check`, also install clang 20, which is
the version CI pins. `./dev` looks up the versioned tools directly:

```sh
sudo apt install clang-format-20 clang-tidy-20
```

Keep the version aligned with CI. `clang-format` output changes between
releases, so a different version can fail the formatting check, and clang 18
cannot see `std::expected` in the C++23 standard library headers: with clang 18,
`./dev check` stops at the clang-tidy step with `no template named 'expected' in
namespace 'std'` for every source file.

During development, configure once, then build and run the affected suite.
This example runs the agent tests:

```sh
./dev configure
./dev test --suite agent
```

Run the full validation pipeline before a release or after major build and
toolchain changes:

```sh
./dev check
```

It checks formatting, builds the project, runs clang-tidy and all unit tests,
then builds and tests with ASan/UBSan. This full pipeline is more expensive than
the focused build and test loop above.

## Next steps

- [Quickstart](/guides/quickstart/) for provider setup and your first turn
- [Configuration](/guides/configuration/) for `~/.niminal/config.json`
