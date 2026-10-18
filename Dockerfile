# syntax=docker/dockerfile:1.7
FROM --platform=$BUILDPLATFORM rust:1.99.0-trixie@sha256:81015672d6939b0265f09adf96d5de125482bb1330106076956b0518e74edc90 AS build
ARG ZIG_VERSION=0.17.0
ARG ZIG_SHA256_AMD64=1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026
ARG ZIG_SHA256_ARM64=9e8d11661d4ae3bd57702a3832781e23ad151dde5798e16a5ccd503f65234ff8
ARG BUILDARCH
RUN set -eux; \
    case "$BUILDARCH" in \
      amd64) arch=x86_64; sum=$ZIG_SHA256_AMD64 ;; \
      arm64) arch=aarch64; sum=$ZIG_SHA256_ARM64 ;; \
      *) echo "unsupported build platform $BUILDARCH"; exit 1 ;; \
    esac; \
    curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-${arch}-linux-${ZIG_VERSION}.tar.xz" -o /tmp/zig.tar.xz; \
    echo "${sum}  /tmp/zig.tar.xz" | sha256sum -c -; \
    mkdir /opt/zig && tar -xJf /tmp/zig.tar.xz -C /opt/zig --strip-components=1; \
    ln -s /opt/zig/zig /usr/local/bin/zig; \
    rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl; \
    cargo install --locked cargo-zigbuild@0.23.4
WORKDIR /src
COPY . .
ARG TARGETARCH
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,id=burrow-target-${TARGETARCH} \
    set -eux; \
    case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-musl ;; \
      arm64) target=aarch64-unknown-linux-musl; export JEMALLOC_SYS_WITH_LG_PAGE=16 ;; \
      *) echo "unsupported target $TARGETARCH"; exit 1 ;; \
    esac; \
    cargo zigbuild --locked --release --target "$target"; \
    install -D "target/$target/release/burrow" /out/burrow

FROM gcr.io/distroless/static-debian13:nonroot@sha256:e2e927ec666bae08560abb3c55d0659eceabb657f56b6782ab500a9fc7f555e3
COPY --from=build /out/burrow /usr/local/bin/burrow
USER nonroot:nonroot
WORKDIR /etc/burrow
EXPOSE 7835 7836
ENTRYPOINT ["/usr/local/bin/burrow"]
CMD ["server", "-c", "/etc/burrow/server.yaml"]
