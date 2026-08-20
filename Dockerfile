# make docker
# make docker PLATFORM=linux/arm64 GOBGP_VERSION=v4.3.0

FROM rust:1-alpine AS rust-builder
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY service ./service
ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN cargo build --release && cp target/release/gobgp-sync /gobgp-sync

FROM golang:1-alpine AS gobgpd-builder
ARG GOBGP_VERSION=latest
RUN apk add --no-cache git
RUN set -e; \
    case "$GOBGP_VERSION" in \
      latest) TAG=$(git ls-remote --tags --refs https://github.com/osrg/gobgp.git \
                | sed 's|.*/||' | grep -E '^v4\.[0-9]+\.[0-9]+$' \
                | sort -t. -k1.2,1n -k2,2n -k3,3n | tail -1) ;; \
      v*) TAG=$GOBGP_VERSION ;; \
      *)  TAG=v$GOBGP_VERSION ;; \
    esac; \
    git clone --depth 1 --branch "$TAG" https://github.com/osrg/gobgp.git /src; \
    cd /src && CGO_ENABLED=0 go build -trimpath -ldflags='-s -w' -o /gobgpd ./cmd/gobgpd

FROM scratch
WORKDIR /etc/gobgp-sync
COPY --from=rust-builder /gobgp-sync ./bin/gobgp-sync
COPY --from=gobgpd-builder /gobgpd ./bin/gobgpd
COPY config ./config
ENTRYPOINT ["/etc/gobgp-sync/bin/gobgp-sync"]
CMD ["-c", "config/config.toml", "--gobgpd-config", "config/gobgpd.conf"]
