FROM rust:1.99.1-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --locked --release -p meibo-app

FROM debian:bookworm-slim AS runtime
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

FROM node:24.22.3-bookworm-slim AS connector-node
FROM runtime AS activity-sync
USER root
COPY --from=connector-node /usr/local/bin/node /usr/local/bin/node
COPY --from=connector-node /usr/local/LICENSE /usr/local/share/doc/node/LICENSE
RUN /usr/local/bin/node --version \
    && install -d -m 0700 -o 10001 -g 10001 /var/lib/meibo-activity
ENV MEIBO_ACTIVITY_SYNC_NODE=/usr/local/bin/node \
    MEIBO_SYNC_VAULT=/var/lib/meibo-activity
USER 10001:10001

# 公開デモ用（`docker build --target demo`）。起動のたびに新しい作業用ディレクトリで、
# 合成データだけの使い捨てのデモを始める。MEIBO_DOMAIN（公開ドメイン）は必須、
# PORT は実行先が決める（既定4317）、MEIBO_DEMO_CONTACT は入口に出す連絡先URL（任意）。
FROM runtime AS demo
ENV PORT=4317
ENTRYPOINT ["/bin/sh", "-c", "exec /usr/local/bin/meibo hosted \"/tmp/meibo-demo-$(date +%s)-$$\" serve --public-demo --origin \"https://${MEIBO_DOMAIN:?MEIBO_DOMAIN is required}\" --bind \"0.0.0.0:${PORT}\" ${MEIBO_DEMO_CONTACT:+--demo-contact \"${MEIBO_DEMO_CONTACT}\"}"]
CMD []

# Keep the default build free of connector runtimes and external account configuration.
FROM runtime AS default
