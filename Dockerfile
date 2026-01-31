# syntax=docker/dockerfile:1.6

ARG NODE_VERSION=20-bookworm-slim
ARG RUST_VERSION=1.91-bookworm
ARG BUF_VERSION=1.60.0

FROM node:${NODE_VERSION} AS web-build
WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
      ca-certificates \
      curl \
    && rm -rf /var/lib/apt/lists/*

ARG BUF_VERSION
RUN curl -sSL "https://github.com/bufbuild/buf/releases/download/v${BUF_VERSION}/buf-Linux-x86_64" -o /usr/local/bin/buf \
    && chmod +x /usr/local/bin/buf \
    && buf --version

COPY web/package.json web/package-lock.json ./web/
RUN cd web && npm ci

COPY buf.yaml buf.lock buf.gen.yaml ./
COPY proto/ ./proto/
COPY web/ ./web/

RUN buf generate
RUN cd web && npm run build

FROM rust:${RUST_VERSION} AS rust-build
WORKDIR /src

# Build dependencies (buf + OpenSSL headers for crates that need it)
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
      ca-certificates \
      curl \
      pkg-config \
      libssl-dev \
    && rm -rf /var/lib/apt/lists/*

ARG BUF_VERSION
RUN curl -sSL "https://github.com/bufbuild/buf/releases/download/v${BUF_VERSION}/buf-Linux-x86_64" -o /usr/local/bin/buf \
    && chmod +x /usr/local/bin/buf \
    && buf --version

COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY buf.yaml buf.lock ./
COPY proto/ ./proto/
COPY src/ ./src/

RUN cargo build --release --locked --bin sitebookify-app

FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
      ca-certificates \
      libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -u 10001 app

COPY --from=rust-build /src/target/release/sitebookify-app /usr/local/bin/sitebookify-app
COPY --from=web-build /src/web/dist ./web/dist

EXPOSE 8080
USER 10001

ENTRYPOINT ["/usr/local/bin/sitebookify-app"]
CMD ["--addr","0.0.0.0:8080","--data-dir","/tmp/workspace-app","--web-dir","/app/web/dist"]
