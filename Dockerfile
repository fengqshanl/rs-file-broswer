ARG RUST_IMAGE=rust:1-alpine
ARG ALPINE_IMAGE=alpine:3.20

FROM ${RUST_IMAGE} AS builder

RUN apk add --no-cache build-base perl
WORKDIR /build

# Build dependencies first so source edits do not invalidate the dependency layer.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
    && printf 'fn main() {}\n' > src/main.rs \
    && cargo build --release --locked

COPY src ./src
COPY static ./static
RUN touch src/main.rs && cargo build --release --locked

FROM ${ALPINE_IMAGE}

# ca-certificates is only used by the busybox-wget container healthcheck.
RUN apk add --no-cache ca-certificates

COPY --from=builder /build/target/release/agentgod-files /usr/local/bin/agentgod-files

# Runs as root on purpose: the mounted host data tree is root-owned and the
# service exists to manage those files. Compose drops every capability except
# the two file-access ones and applies memory/CPU/PID limits.
WORKDIR /data
EXPOSE 8090
ENTRYPOINT ["/usr/local/bin/agentgod-files"]
