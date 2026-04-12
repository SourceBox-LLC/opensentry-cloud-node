# Build stage
FROM rust:1.80-alpine AS builder

RUN apk add --no-cache musl-dev

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
FROM alpine:3.19

# Install FFmpeg for HLS generation and libv4l for USB cameras
RUN apk add --no-cache \
    ffmpeg \
    libv4l \
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

ENTRYPOINT ["opensentry-cloudnode"]