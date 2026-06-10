# Build stage: static musl binary (matches the MSRV in Cargo.toml)
FROM rust:1.88-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

# Runtime stage: minimal image with CA certificates for ASN/IX/update lookups
FROM alpine:3.21
RUN apk add --no-cache ca-certificates
COPY --from=build /src/target/release/ttl /usr/local/bin/ttl

# ttl needs CAP_NET_RAW for ICMP sockets. Docker grants NET_RAW by default;
# stricter runtimes need: --cap-add NET_RAW
ENTRYPOINT ["ttl"]
CMD ["--help"]

# Example healthcheck when running with --prometheus :9090
# (uncomment, or set in compose/orchestration):
# HEALTHCHECK --interval=30s --timeout=3s \
#   CMD wget -qO- http://127.0.0.1:9090/healthz || exit 1
