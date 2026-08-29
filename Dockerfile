# syntax=docker/dockerfile:1

FROM rust:bookworm AS builder

WORKDIR /build
COPY . .
RUN cargo build --locked --release -p file-converter-gateway

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home converter

COPY --from=builder /build/target/release/file-converter-gateway /usr/local/bin/file-converter-gateway

USER converter

ENV BIND_ADDRESS=0.0.0.0:8080 \
    GOTENBERG_URL=http://gotenberg:3000 \
    RUST_LOG=file_converter_gateway=info,tower_http=info

EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/file-converter-gateway"]
