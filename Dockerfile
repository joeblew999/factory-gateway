# Build the gateway binary (fetches factory-machine-model + factory-howick-driver from GitHub).
FROM rust:1.95-slim-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev git ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN cargo build --release --bin factory-gateway

# Slim runtime image.
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
        && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/factory-gateway /usr/local/bin/factory-gateway
EXPOSE 4840 4841
ENTRYPOINT ["factory-gateway"]
CMD ["--config", "/config/gateway.toml"]
