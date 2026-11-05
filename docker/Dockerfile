# syntax=docker/dockerfile:1.7
#
# A chainv4 node built with every feature, with the explorer's and the wallet's web apps for it to
# serve from / and /wallet/. Built from the repository's root:
#
#   docker build -f docker/Dockerfile -t chainv4 .
#   podman build --format docker -f docker/Dockerfile -t chainv4 .
#
# docker/compose.yaml runs it behind nginx, with Docker or Podman (docker/compose.sh); see
# docs/docker.md. Images are named with their registry, as Podman wants them.

# The toolchains, pinned (see docs/development.md, "Toolchains"): Rust as rust-toolchain.toml names
# it, which tests/toolchains.rs checks, and clang as docker/llvm.sh installs it, LLVM's own release
# by its SHA-256, which compiles the C and C++ that Luau's metering and RocksDB are made of. The CI
# runs the same script. Debian's own cc is GCC, so CC and CXX name clang.
ARG RUST_VERSION=1.98.1
ARG DEBIAN=bookworm
ARG TRUNK_VERSION=0.22.0

# The node. RocksDB and Luau build from source, by clang, and bindgen needs its libclang.
FROM docker.io/library/rust:${RUST_VERSION}-${DEBIAN} AS node
COPY docker/llvm.sh /usr/local/lib/llvm.sh
RUN sh /usr/local/lib/llvm.sh /opt/llvm && rustc --version
# bindgen asks the clang on PATH where clang's own headers are, so libclang finds stdbool.h.
ENV CC=/opt/llvm/bin/clang \
    CXX=/opt/llvm/bin/clang++ \
    LIBCLANG_PATH=/opt/llvm/lib \
    PATH=/opt/llvm/bin:${PATH}
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
    && cp target/release/chainv4 /usr/local/bin/chainv4

# The web apps, built with trunk, which fetches wasm-bindgen and wasm-opt to suit. Its static musl
# build, as the glibc one wants a newer glibc than Debian's.
FROM docker.io/library/rust:${RUST_VERSION}-${DEBIAN} AS web
ARG TRUNK_VERSION
ARG TARGETARCH
RUN rustup target add wasm32-unknown-unknown
RUN set -eu; \
    target=${TARGETARCH:-$(dpkg --print-architecture)}; \
    case "$target" in arm64) arch=aarch64 ;; amd64) arch=x86_64 ;; *) echo "no trunk for $target" >&2; exit 1 ;; esac; \
    file="trunk-$arch-unknown-linux-musl.tar.gz"; \
    url="https://github.com/trunk-rs/trunk/releases/download/v$TRUNK_VERSION/$file"; \
    cd /tmp; \
    curl -fsSLO "$url"; \
    curl -fsSL "$url.sha256" | awk '{ print $1 "  '"$file"'" }' | sha256sum -c -; \
    tar -xzf "$file" -C /usr/local/bin trunk; \
    rm "$file"
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/explorer/target \
    --mount=type=cache,target=/src/wallet/target \
    --mount=type=cache,target=/root/.cache/trunk \
    cd /src/explorer && trunk build --release \
    && cd /src/wallet && trunk build --release

FROM docker.io/library/debian:${DEBIAN}-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl jq \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --no-create-home --home-dir /data --shell /usr/sbin/nologin chainv4 \
    && mkdir -p /data \
    && chown chainv4 /data
COPY --from=node /usr/local/bin/chainv4 /usr/local/bin/chainv4
COPY --from=web /src/explorer/dist /opt/chainv4/explorer
COPY --from=web /src/wallet/dist /opt/chainv4/wallet
COPY --chmod=0755 docker/entrypoint.sh /usr/local/bin/chainv4-entrypoint
USER chainv4
WORKDIR /data
VOLUME /data
# The API, for nginx in front, and peer-to-peer over QUIC.
EXPOSE 8080/tcp 4001/udp
HEALTHCHECK --interval=10s --start-period=20s CMD curl -fs http://127.0.0.1:8080/status > /dev/null || exit 1
ENTRYPOINT ["chainv4-entrypoint"]
