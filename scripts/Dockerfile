# Cross-platform builds from any host: `make linux-docker`, `make windows-docker`.
# target/ and the crate registry live in cache mounts: reused by every build, never baked
# into image layers, so the build cache holds one copy instead of one per source change.
ARG IMAGE=rust:1-bookworm
FROM ${IMAGE} AS base
WORKDIR /src
# Optional extra CAs (TLS-inspecting proxies): `make docker-ca` writes .docker-ca.pem.
COPY .dockerignore .docker-ca.pe[m] /tmp/ca/
RUN if [ -f /tmp/ca/.docker-ca.pem ]; then \
      cp /tmp/ca/.docker-ca.pem /usr/local/share/ca-certificates/extra.crt && update-ca-certificates >/dev/null; \
    fi

FROM base AS linux
# gpui (X11/Wayland/Vulkan/fonts) + xcap (xcb, PipeWire via bindgen -> libclang).
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
      libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libxcb1-dev libxcb-randr0-dev \
      libfontconfig-dev libfreetype-dev libvulkan-dev libasound2-dev libzstd-dev \
      libpipewire-0.3-dev libclang-dev clang libdbus-1-dev libegl-dev libgbm-dev pkg-config >/dev/null
COPY . .
RUN --mount=type=cache,id=rusty-beaver-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=rusty-beaver-linux-target,target=/src/target \
    cargo build --release && cargo test --release && cp target/release/rusty-beaver /rusty-beaver

FROM base AS windows
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
      gcc-mingw-w64-x86-64 binutils-mingw-w64-x86-64 >/dev/null \
 && rustup target add x86_64-pc-windows-gnu
COPY . .
RUN --mount=type=cache,id=rusty-beaver-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=rusty-beaver-windows-target,target=/src/target \
    cargo build --profile release-cross --target x86_64-pc-windows-gnu \
 && cp target/x86_64-pc-windows-gnu/release-cross/rusty-beaver.exe /rusty-beaver.exe
