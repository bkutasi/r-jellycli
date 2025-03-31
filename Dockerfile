FROM rust:latest AS builder

WORKDIR /usr/src/r-jellycli

# Copy only necessary files for dependency resolution
COPY Cargo.toml Cargo.lock ./

# Create dummy main.rs to build dependencies
RUN mkdir -p src && \
    echo "fn main() { println!(\"dummy\"); }" > src/main.rs

# Build dependencies (cached layer)
RUN apt-get update && \
    apt-get install -y pkg-config libasound2-dev && \
    cargo build --release --bin r-jellycli && \
    rm -rf /var/lib/apt/lists/*

# Remove the dummy source code
RUN rm -rf src

# Copy actual source code
COPY src ./src

# Build the actual application
RUN cargo build --release --bin r-jellycli

# Create a slim runtime image
FROM debian:bookworm-slim

# Install only runtime dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends libasound2 libssl3 ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Create app directory and config directory
WORKDIR /app
RUN mkdir -p /root/.config/r-jellycli/

# Copy only the binary from the builder stage
COPY --from=builder /usr/src/r-jellycli/target/release/r-jellycli /app/

# Command to run
ENTRYPOINT ["/app/r-jellycli"]
