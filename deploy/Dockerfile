# syntax=docker/dockerfile:1
FROM ubuntu:24.04

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates tzdata curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY target/release/aion_session_store /app/aion_session_store

# Run as non-root user 1000:1000
USER 1000:1000

ENV PORT=8950
EXPOSE 8950

ENTRYPOINT ["/app/aion_session_store"]
