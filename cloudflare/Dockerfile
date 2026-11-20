FROM --platform=$BUILDPLATFORM rust:1.99.1-bookworm AS build
ARG BUILDARCH
ARG TARGETARCH
WORKDIR /build
# Compile the native dependencies with host executables. Running the amd64 C
# compiler under QEMU can crash while assembling aws-lc on arm64 builders.
RUN set -eu; \
    if [ "$BUILDARCH" = "$TARGETARCH" ]; then \
        set -- libssl-dev; \
    elif [ "$BUILDARCH/$TARGETARCH" = "arm64/amd64" ]; then \
        dpkg --add-architecture amd64; \
        rustup target add x86_64-unknown-linux-gnu; \
        set -- gcc-x86-64-linux-gnu g++-x86-64-linux-gnu libc6-dev-amd64-cross libssl-dev:amd64; \
    else \
        echo "Unsupported cross-build: $BUILDARCH to $TARGETARCH" >&2; exit 1; \
    fi; \
    apt-get update; \
    apt-get install --yes --no-install-recommends pkg-config cmake "$@"; \
    rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN set -eu; \
    binary=target/release/meibo; \
    if [ "$BUILDARCH" != "$TARGETARCH" ]; then \
        export CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu \
            CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
            CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
            CXX_x86_64_unknown_linux_gnu=x86_64-linux-gnu-g++ \
            AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar \
            PKG_CONFIG_ALLOW_CROSS_x86_64_unknown_linux_gnu=1 \
            PKG_CONFIG_LIBDIR_x86_64_unknown_linux_gnu=/usr/lib/x86_64-linux-gnu/pkgconfig; \
        binary="target/$CARGO_BUILD_TARGET/release/meibo"; \
    fi; \
    RUSTFLAGS="-C linker-features=-lld" CARGO_BUILD_JOBS=2 cargo build --locked --release -p meibo-app --features cloud; \
    install -m 0755 "$binary" /usr/local/bin/meibo; \
    if [ "$TARGETARCH" = amd64 ]; then \
        LC_ALL=C readelf -h /usr/local/bin/meibo | grep -q 'Machine:.*X86-64'; \
    fi

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends libssl3 ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 meibo \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin meibo
COPY --from=build /usr/local/bin/meibo /usr/local/bin/meibo
COPY --chmod=0555 cloudflare/entrypoint.sh /usr/local/bin/meibo-cloud-entrypoint
ARG MEIBO_SOURCE_REVISION
LABEL org.opencontainers.image.revision=$MEIBO_SOURCE_REVISION
USER 10001:10001
WORKDIR /tmp
EXPOSE 4317
ENTRYPOINT ["/usr/local/bin/meibo-cloud-entrypoint"]
