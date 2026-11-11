# syntax=docker/dockerfile:1
#
# cavea-server with the console compiled in (08 §14, 09 §9): x86-64 and arm64.
#
#   docker buildx build -f deploy/Dockerfile -t cavea-server .
#   docker buildx build -f deploy/Dockerfile --platform linux/arm64 -t cavea-server:arm64 .
#
# The build stage runs on the build machine's own platform and cross-compiles
# the server for the target's, so arm64 needs no emulation. Tool versions are
# the repository's: Rust from rust-toolchain.toml (rustup reads it), bun from
# package.json's packageManager (checked below), binaryen 132 as in CI
# (.github/workflows/ci.yml) and flake.nix. Bump them together.

ARG BUN_VERSION=1.4.2
ARG BINARYEN_VERSION=132

# rustup and Debian 12's glibc, which the distroless base below shares.
FROM --platform=$BUILDPLATFORM docker.io/library/rust:1.91.1-bookworm@sha256:c1e5f19e773b7878c3f7a805dd00a495e747acbdc76fb2337a4ebf0418896b33 AS build
ARG BUN_VERSION
ARG BINARYEN_VERSION
ARG BUILDARCH
ARG TARGETARCH
# Like every cargo build on a shared machine (AGENTS.md).
ARG CARGO_BUILD_JOBS=8
ENV CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS

# A C cross compiler for the target (ring's C and assembly, and the link);
# nothing for a native build, where Debian's own GCC answers to both names.
RUN set -eu; \
    if [ "$TARGETARCH" != "$BUILDARCH" ]; then \
      case "$TARGETARCH" in \
        arm64) packages="gcc-aarch64-linux-gnu libc6-dev-arm64-cross" ;; \
        amd64) packages="gcc-x86-64-linux-gnu libc6-dev-amd64-cross" ;; \
        *) echo "unsupported target platform $TARGETARCH" >&2; exit 1 ;; \
      esac; \
      apt-get update && apt-get install -y --no-install-recommends $packages \
      && rm -rf /var/lib/apt/lists/*; \
    fi
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc

# bun and wasm-opt, checked against their published SHA-256.
RUN set -eu; \
    case "$BUILDARCH" in \
      amd64) bun=bun-linux-x64; bun_sha=36368faef7527875d5ffa52e53cd48021741f2a83eb6208a8dd64068d422a913; \
             bin=x86_64-linux; bin_sha=195ddc94f9bc89f45abdabb0b9eea86023d727ba90eac8b35b80f2544fc30572 ;; \
      arm64) bun=bun-linux-aarch64; bun_sha=54328bbc2d9c8e0c9f892c544d66c57a83b84139e34909e5ee81758f1ac8fda7; \
             bin=aarch64-linux; bin_sha=c58562417836c5d0493d89bdefc434933bdc097db641b483df86bcfa557a107f ;; \
      *) echo "unsupported build platform $BUILDARCH" >&2; exit 1 ;; \
    esac; \
    curl -sSfL -o /tmp/bun.zip "https://github.com/oven-sh/bun/releases/download/bun-v$BUN_VERSION/$bun.zip"; \
    echo "$bun_sha  /tmp/bun.zip" | sha256sum -c -; \
    unzip -q -j /tmp/bun.zip "$bun/bun" -d /usr/local/bin; \
    curl -sSfL -o /tmp/binaryen.tar.gz "https://github.com/WebAssembly/binaryen/releases/download/version_$BINARYEN_VERSION/binaryen-version_$BINARYEN_VERSION-$bin.tar.gz"; \
    echo "$bin_sha  /tmp/binaryen.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/binaryen.tar.gz -C /usr/local/bin --strip-components=2 "binaryen-version_$BINARYEN_VERSION/bin/wasm-opt"; \
    rm /tmp/bun.zip /tmp/binaryen.tar.gz

WORKDIR /src
# The toolchain, its wasm32 target and the cross target, in a layer of their own.
COPY rust-toolchain.toml ./
RUN rustup toolchain install; \
    if [ "$TARGETARCH" = arm64 ]; then rustup target add aarch64-unknown-linux-gnu; \
    else rustup target add x86_64-unknown-linux-gnu; fi

COPY . .
RUN grep -q "\"packageManager\": \"bun@$BUN_VERSION\"" package.json \
    || { echo "BUN_VERSION $BUN_VERSION is not package.json's packageManager" >&2; exit 1; }
# The registry, the target directory and bun's cache persist between builds,
# so a change to the sources recompiles only what it touches.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    --mount=type=cache,target=/root/.bun/install/cache \
    set -eu; \
    case "$TARGETARCH" in \
      amd64) triple=x86_64-unknown-linux-gnu ;; \
      arm64) triple=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported target platform $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    bun install --frozen-lockfile; \
    cargo xtask build-server --target "$triple"; \
    install -D "target/$triple/release/cavea-server" /out/cavea-server

# glibc, CA certificates and a nonroot user (65532); no shell.
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f
COPY --from=build /out/cavea-server /usr/local/bin/cavea-server
USER nonroot:nonroot
# All interfaces inside the container; publish the port as the deployment needs.
ENV CAVEA_LISTEN=0.0.0.0:8080
EXPOSE 8080
# The server drains and exits within 20 s of SIGTERM: give it 30 s
# (`docker stop -t 30`, compose `stop_grace_period: 30s`).
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=10s --timeout=3s --start-period=30s --retries=3 \
    CMD ["/usr/local/bin/cavea-server", "probe"]
ENTRYPOINT ["/usr/local/bin/cavea-server"]
