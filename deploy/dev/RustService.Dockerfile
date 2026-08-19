ARG RUST_IMAGE=rust@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663
ARG RUNTIME_IMAGE=mcr.microsoft.com/dotnet/runtime-deps@sha256:c62d6267bf8f029da10d716163c274b158f5594b6cc7ee125a08efd64e776df6
FROM ${RUST_IMAGE} AS builder

ARG BINARY
ARG FICANT_CODE_COMMIT_SHA
ARG FICANT_CODE_TREE_SHA
ENV RUSTUP_TOOLCHAIN=1.96.1-x86_64-unknown-linux-gnu \
    CARGO_HTTP_MULTIPLEXING=false \
    CARGO_NET_RETRY=10 \
    FICANT_CODE_COMMIT_SHA=${FICANT_CODE_COMMIT_SHA} \
    FICANT_CODE_TREE_SHA=${FICANT_CODE_TREE_SHA}
WORKDIR /workspace
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY binaries ./binaries
COPY crates ./crates
COPY cpp ./cpp
COPY interface ./interface
RUN --mount=type=cache,id=ficant-cargo-registry-v1,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=ficant-release-target-v1,target=/workspace/target,sharing=locked \
    cargo \
        --config 'source.crates-io.replace-with="github-index"' \
        --config 'source.github-index.registry="sparse+https://raw.githubusercontent.com/rust-lang/crates.io-index/master/"' \
        build --locked --release --bin "${BINARY}" \
    && install -D -m 0755 "target/release/${BINARY}" /out/ficant

FROM ${RUNTIME_IMAGE}

COPY --from=builder /out/ficant /usr/local/bin/ficant
USER 1654:1654
EXPOSE 8080
HEALTHCHECK --interval=5s --timeout=3s --start-period=2s --retries=12 \
    CMD ["/usr/local/bin/ficant", "--health-check"]
ENTRYPOINT ["/usr/local/bin/ficant"]
