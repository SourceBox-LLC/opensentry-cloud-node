# Build stage
FROM rust:1-alpine AS builder

RUN apk add --no-cache musl-dev perl make

WORKDIR /app

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src target/release/opensentry-cloudnode*

# Build application
COPY src ./src
RUN cargo build --release

# Runtime stage
FROM alpine:3.21

# Install FFmpeg for HLS generation and v4l-utils for USB camera
# diagnostics (v4l2-ctl).  We don't link libv4l — the Linux camera
# code uses raw v4l2 ioctls via libc — but the `v4l-utils` package is
# handy inside the container for `docker exec … v4l2-ctl --list-devices`
# when a user reports "camera not detected" in a containerized deploy.
# Alpine has no `libv4l` package; the shared libraries live inside
# `v4l-utils-libs` which is pulled in transitively by `v4l-utils`.
RUN apk add --no-cache \
    ffmpeg \
    v4l-utils

# Create non-root user
RUN adduser -D -s /bin/sh opensentry

WORKDIR /app

# Copy binary
COPY --from=builder /app/target/release/opensentry-cloudnode /usr/local/bin/

# Create storage directories
RUN mkdir -p /app/data/hls && \
    mkdir -p /app/data/recordings && \
    mkdir -p /app/data/snapshots && \
    chown -R opensentry:opensentry /app/data

USER opensentry

# Expose HTTP server port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD wget --no-verbose --tries=1 --spider http://localhost:8080/health || exit 1

# Volume for persistence
VOLUME ["/app/data"]

# Tell CloudNode where to persist its fallback machine-ID when /etc/machine-id
# isn't bind-mounted from the host. The ID lives in the volume so it survives
# container rebuilds. For stronger encryption (key tied to host, not data
# volume), run with `-v /etc/machine-id:/etc/machine-id:ro`.
ENV OPENSENTRY_DATA_DIR=/app/data

ENTRYPOINT ["opensentry-cloudnode"]