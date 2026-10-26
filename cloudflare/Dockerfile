FROM rust:1.99.1-bookworm AS build
WORKDIR /build
RUN apt-get update \
    && apt-get install --yes --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --locked --release -p meibo-app --features cloud

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends libssl3 ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 meibo \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin meibo
COPY --from=build /build/target/release/meibo /usr/local/bin/meibo
COPY --chmod=0555 cloudflare/entrypoint.sh /usr/local/bin/meibo-cloud-entrypoint
USER 10001:10001
WORKDIR /tmp
EXPOSE 4317
ENTRYPOINT ["/usr/local/bin/meibo-cloud-entrypoint"]
