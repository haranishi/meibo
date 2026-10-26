FROM rust:1.99.1-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --locked --release -p meibo-app

FROM debian:bookworm-slim
RUN groupadd --gid 10001 meibo \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin meibo \
    && mkdir /data \
    && chown 10001:10001 /data \
    && chmod 700 /data
COPY --from=build /build/target/release/meibo /usr/local/bin/meibo
USER 10001:10001
WORKDIR /data
ENTRYPOINT ["/usr/local/bin/meibo"]
CMD ["--help"]
