# Builds a Linux niminal binary and a small runtime image.
#
#   docker build -t niminal:local .
#   docker run --rm -it -e OPENROUTER_API_KEY niminal:local bash
#
# The image targets the build host architecture. To build the x86_64 release
# binary on Apple silicon, add --platform linux/amd64 to both commands.
#
# The compose file in the parent directory wires this up with a workspace mount
# and host environment passthrough.

FROM ubuntu:24.04 AS build

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        cmake \
        g++ \
        git \
        libssl-dev \
        ninja-build \
        zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .
RUN cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release \
    && cmake --build build --target niminal_cli --parallel

FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        git \
        libssl3t64 \
        python3 \
        zlib1g \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/build/niminal /usr/local/bin/niminal

WORKDIR /workspace
CMD ["bash"]
