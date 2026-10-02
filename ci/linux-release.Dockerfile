FROM rockylinux:8 AS toolchain

RUN dnf install -y --setopt=install_weak_deps=False \
      gcc-toolset-15-gcc-c++ git tar gzip binutils perl make ca-certificates \
    && dnf clean all

RUN set -eu; \
    case "$(uname -m)" in \
      aarch64) arch=aarch64; sha=a343c6294f770742904e6a6792e0956b5ff8212abfb63cac99237de2e210fa0f ;; \
      x86_64) arch=x86_64; sha=3cb3dd247b6a1de2d0f4b20c6fd4326c9024e894cebc9dc8699758887e566ca7 ;; \
      *) exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/cmake.tar.gz \
      "https://github.com/Kitware/CMake/releases/download/v3.31.10/cmake-3.31.10-linux-${arch}.tar.gz"; \
    echo "$sha  /tmp/cmake.tar.gz" | sha256sum -c -; \
    mkdir -p /opt/tools; \
    tar -xzf /tmp/cmake.tar.gz -C /opt/tools --strip-components=1; \
    rm /tmp/cmake.tar.gz

# Upstream OpenSSL cannot parse every distribution's patched crypto-policy config.
# Keep its default config private; CAIL loads the target system's CA certificates.
RUN set -eu; \
    curl -fsSL -o /tmp/openssl.tar.gz \
      https://github.com/openssl/openssl/releases/download/openssl-3.5.8/openssl-3.5.8.tar.gz; \
    echo 'a8f84a39918ec6415ce765d9b429d313ba97b8143169c172e734b9514464f5b2  /tmp/openssl.tar.gz' | sha256sum -c -; \
    tar -xzf /tmp/openssl.tar.gz -C /tmp; \
    cd /tmp/openssl-3.5.8; \
    . /opt/rh/gcc-toolset-15/enable; \
    ./Configure no-shared no-tests no-apps no-zlib --prefix=/opt/openssl --libdir=lib --openssldir=/opt/niminal/ssl; \
    make --silent -j4; \
    make --silent install_sw; \
    rm -rf /tmp/openssl-3.5.8 /tmp/openssl.tar.gz

ENV PATH=/opt/tools/bin:$PATH
ENV CMAKE_PREFIX_PATH=/opt/cail:/opt/openssl

FROM toolchain AS build
COPY .ci/cail /work/cail
RUN . /opt/rh/gcc-toolset-15/enable \
    && cmake -S /work/cail -B /work/cail-build \
      -DCMAKE_BUILD_TYPE=Release -DCAIL_BUILD_EXAMPLES=OFF -DBUILD_TESTING=OFF \
      -DOPENSSL_ROOT_DIR=/opt/openssl -DOPENSSL_USE_STATIC_LIBS=TRUE \
    && cmake --build /work/cail-build --parallel 4 \
    && cmake --install /work/cail-build --prefix /opt/cail

COPY . /work/niminal
RUN . /opt/rh/gcc-toolset-15/enable \
    && cmake -S /work/niminal -B /work/build \
      -DCMAKE_BUILD_TYPE=Release -DCMAKE_C_COMPILER=gcc -DCMAKE_CXX_COMPILER=g++ \
      -DCMAKE_CXX_STANDARD=23 -DCMAKE_CXX_STANDARD_REQUIRED=ON \
      -DOPENSSL_ROOT_DIR=/opt/openssl \
      '-DCMAKE_EXE_LINKER_FLAGS=-static-libstdc++ -static-libgcc' \
    && cmake --build /work/build --target niminal_cli --parallel 4 \
    && /work/build/niminal --version \
    && floor=$(objdump --dynamic-syms /work/build/niminal | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -Vu | tail -1) \
    && test "$(printf '%s\n' 2.27 "$floor" | sort -V | tail -1)" = 2.27 \
    && ! objdump -p /work/build/niminal | grep NEEDED | grep -E 'lib(ssl|crypto|stdc\+\+|gcc_s)'

FROM scratch AS artifact
COPY --from=build /work/build/niminal /niminal
