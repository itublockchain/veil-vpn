FROM rust:1.78-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY boringtun/ boringtun/
COPY boringtun-cli/ boringtun-cli/

RUN cargo build --release --features payment

# ---------- Runtime ----------
FROM node:20-bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    iproute2 iptables ca-certificates socat curl openssl xxd \
    && rm -rf /var/lib/apt/lists/*

# Copy boringtun binary
COPY --from=builder /build/target/release/boringtun-cli /usr/local/bin/boringtun

# Install tee-service deps
WORKDIR /app/tee-service
COPY tee-service/package.json tee-service/tsconfig.json ./
RUN npm install --production=false

COPY tee-service/src/ src/

# Entrypoint script
COPY tee-service/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

VOLUME /app/data
EXPOSE 8080 8443 51820/udp

ENTRYPOINT ["/entrypoint.sh"]
