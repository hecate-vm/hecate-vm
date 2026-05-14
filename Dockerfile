# ──────────────────────────────────────────────────────────────────────────────
# Stage 1 – chef: install cargo-chef once, reused by planner + builder
# ──────────────────────────────────────────────────────────────────────────────
FROM rust:1-slim-trixie AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

# ──────────────────────────────────────────────────────────────────────────────
# Stage 2 – planner: produce the dependency recipe (only reruns on Cargo.toml
# / Cargo.lock changes, not on src changes)
# ──────────────────────────────────────────────────────────────────────────────
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
# Provide a stub main so chef can inspect the full dependency graph
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo chef prepare --recipe-path recipe.json

# ──────────────────────────────────────────────────────────────────────────────
# Stage 3 – builder: pre-build deps from the recipe, then build the real crate
# ──────────────────────────────────────────────────────────────────────────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json

# Cook (compile) only dependencies – this layer is cached across rebuilds when
# source changes but manifests do not.
RUN cargo chef cook --release --recipe-path recipe.json

# Now copy the real source and do the final compile
COPY . .
RUN cargo build --release --locked

# ──────────────────────────────────────────────────────────────────────────────
# Stage 4 – examples-builder: cross-compile all examples to RV32 ELFs
#
# Based on the chef image (rust:1-slim-trixie + cargo-chef) so Rust and
# rustup are already present.  We add:
#   • clang + lld  – for C/CMake examples targeting riscv32-unknown-elf
#   • cmake        – CMake-based example builds
#   • nightly + rust-src – Rust examples use -Z build-std on a custom target
# ──────────────────────────────────────────────────────────────────────────────
FROM chef AS examples-builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
    clang \
    cmake \
    lld \
    make \
    && rustup component add rust-src --toolchain nightly

COPY . .

# build_samples.sh writes ELFs under target/hecate-rv32-build/examples-build/
RUN bash scripts/build_samples.sh

# Collect every built ELF into a single flat directory for easy COPY later
RUN mkdir -p /built-examples \
    && find target/hecate-rv32-build/examples-build -name '*.elf' \
    -exec cp {} /built-examples/ \;

# ──────────────────────────────────────────────────────────────────────────────
# Stage 5 – runtime: slim Debian image with the VM binary and built examples
# ──────────────────────────────────────────────────────────────────────────────
FROM debian:trixie-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/hecate-vm /usr/local/bin/hecate-vm
COPY --from=examples-builder /built-examples /examples

ENTRYPOINT ["hecate-vm"]
CMD ["--help"]
