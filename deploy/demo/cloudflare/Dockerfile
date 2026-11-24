# 公開デモ用（Cloudflare Containers 向け）。
# リポジトリ直下の Dockerfile の build / runtime / demo ステージと同じ内容を持ち、
# 最終ステージを demo にすることで wrangler がそのままビルドできるようにする。
# ビルドの文脈はリポジトリルート（wrangler.jsonc の image_build_context: "../../.."）。

# ビルドは、実行する側のCPU（BUILDPLATFORM）のままコンパイルし、配備先（TARGETARCH）がamd64で
# ビルドする側がarm64のときだけ、クロスコンパイルする（QEMUの下でRustをコンパイルしない）。
# Apple Siliconのmacで、wranglerが `--platform linux/amd64` でビルドしても、速く確実に作れる。
FROM --platform=$BUILDPLATFORM rust:1.99.1-bookworm AS build
ARG BUILDARCH
ARG TARGETARCH
WORKDIR /build
RUN set -eu; \
    if [ "$BUILDARCH" = "$TARGETARCH" ]; then \
        :; \
    elif [ "$BUILDARCH/$TARGETARCH" = "arm64/amd64" ]; then \
        rustup target add x86_64-unknown-linux-gnu; \
        apt-get update; \
        apt-get install --yes --no-install-recommends gcc-x86-64-linux-gnu libc6-dev-amd64-cross; \
        rm -rf /var/lib/apt/lists/*; \
    else \
        echo "Unsupported cross-build: $BUILDARCH to $TARGETARCH" >&2; exit 1; \
    fi
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN set -eu; \
    binary=target/release/meibo; \
    if [ "$BUILDARCH" != "$TARGETARCH" ]; then \
        export CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu \
            CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
            CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
            AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar; \
        binary="target/$CARGO_BUILD_TARGET/release/meibo"; \
    fi; \
    cargo build --locked --release -p meibo-app; \
    install -m 0755 "$binary" /usr/local/bin/meibo; \
    if [ "$TARGETARCH" = amd64 ]; then \
        LC_ALL=C readelf -h /usr/local/bin/meibo | grep -q 'Machine:.*X86-64'; \
    fi

FROM debian:bookworm-slim AS runtime
RUN groupadd --gid 10001 meibo \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin meibo \
    && mkdir /data \
    && chown 10001:10001 /data \
    && chmod 700 /data
COPY --from=build /usr/local/bin/meibo /usr/local/bin/meibo
USER 10001:10001
WORKDIR /data
ENTRYPOINT ["/usr/local/bin/meibo"]
CMD ["--help"]

# 公開デモ用。起動のたびに新しい作業用ディレクトリで、
# 合成データだけの使い捨てのデモを始める。MEIBO_DOMAIN（公開ドメイン）は必須、
# PORT は実行先が決める（既定4317）、MEIBO_DEMO_CONTACT は入口に出す連絡先URL（任意）。
FROM runtime AS demo
ENV PORT=4317
EXPOSE 4317
ENTRYPOINT ["/bin/sh", "-c", "exec /usr/local/bin/meibo hosted \"/tmp/meibo-demo-$(date +%s)-$$\" serve --public-demo --origin \"https://${MEIBO_DOMAIN:?MEIBO_DOMAIN is required}\" --bind \"0.0.0.0:${PORT}\" ${MEIBO_DEMO_CONTACT:+--demo-contact \"${MEIBO_DEMO_CONTACT}\"}"]
CMD []
