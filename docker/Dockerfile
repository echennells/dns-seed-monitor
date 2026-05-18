# syntax=docker/dockerfile:1.7
FROM rust:1.81-bookworm AS builder
WORKDIR /app
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --bin monitor \
 && cp target/release/monitor /usr/local/bin/monitor

FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=builder /usr/local/bin/monitor /usr/local/bin/monitor
COPY config.toml /app/config.toml
COPY web /app/web
VOLUME ["/app/data"]
EXPOSE 8080
USER nonroot
ENTRYPOINT ["/usr/local/bin/monitor"]
