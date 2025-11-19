# =========================
# 1) Build stage
# =========================
FROM golang:1.24 AS builder

# Create working directory
WORKDIR /app

# Pre-copy go.mod for better layer caching
COPY go.mod ./
RUN go mod download

# Copy the rest of the source
COPY . .

# Let Go resolve deps AND write go.sum
RUN go mod tidy

# Build a statically linked binary
RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -o relay-server .

# =========================
# 2) Runtime stage
# =========================
FROM gcr.io/distroless/static

# This is where local_key.pem will live.
WORKDIR /data

# Copy the built binary
COPY --from=builder /app/relay-server /usr/local/bin/relay-server

# Expose both TCP and UDP on 40142
EXPOSE 40142/tcp
EXPOSE 40142/udp

# `docker run -v relay-data:/data` will persist the key
VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/relay-server"]
