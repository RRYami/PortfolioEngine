# Build context is the workspace root (needs Cargo.lock + all crates).
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# Only ptf-api (+ ptf-engine) is compiled; persistence/tui are not pulled in.
RUN cargo build --release -p ptf-api

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/ptf-api /usr/local/bin/ptf-api
ENV PTF_API_ADDR=0.0.0.0:8080
EXPOSE 8080

HEALTHCHECK --interval=10s --timeout=3s --retries=5 \
    CMD curl -fsS http://localhost:8080/health || exit 1

CMD ["ptf-api"]
