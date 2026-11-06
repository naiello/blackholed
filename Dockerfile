# Build stage
FROM rust:1.92-slim AS builder

WORKDIR /build

# Install build dependencies
RUN apt-get update && \
    apt-get install -y pkg-config libssl-dev perl make && \
    rm -rf /var/lib/apt/lists/*

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Copy source code
COPY src ./src
COPY templates ./templates

# Build release binary
RUN cargo build --release

# Runtime stage
FROM debian:trixie-slim

WORKDIR /app

# Install runtime dependencies
# Distrust the emSign roots NSS 3.131.1 removed after the eMudhra intermediate compromise
# (CVE-2026-111943). Debian's 20250419+deb13u1 only drops G3; remove once it drops C3/C1 too.
RUN apt-get update && \
    apt-get install -y ca-certificates dnsutils && \
    sed -i -E 's#^(mozilla/(emSign_ECC_Root_CA_-_C3|emSign_ECC_Root_CA_-_G3|emSign_Root_CA_-_C1)\.crt)$#!\1#' /etc/ca-certificates.conf && \
    update-ca-certificates --fresh && \
    rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /build/target/release/blackholed /usr/local/bin/blackholed

# Create directory for database
RUN mkdir -p /data

# Expose DNS ports (UDP and TCP) and web API port
EXPOSE 53/udp 53/tcp 5355/tcp

# Set working directory for database
WORKDIR /data

CMD ["blackholed"]
