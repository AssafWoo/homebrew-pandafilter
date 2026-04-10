# PandaFilter integration test image — single stage to avoid GLIBC version mismatch.
# rust:latest provides the same GLIBC as the build output.
FROM rust:latest

RUN apt-get update && apt-get install -y --no-install-recommends \
    git jq sqlite3 curl python3 make bash ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

# Build the release binary once
RUN cargo build --release -p panda 2>&1

# Install the binary system-wide
RUN cp target/release/panda /usr/local/bin/panda && chmod +x /usr/local/bin/panda

# Set up a non-root user to simulate a real dev environment
RUN useradd -m -s /bin/bash testuser
RUN chmod +x /src/docker/test.sh

USER testuser
WORKDIR /home/testuser

ENTRYPOINT ["/src/docker/test.sh"]
