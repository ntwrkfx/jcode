FROM rust:1.96-bookworm

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      bubblewrap ca-certificates git jq pkg-config libssl-dev zstd \
 && rm -rf /var/lib/apt/lists/*

COPY build-project-executor.sh /usr/local/bin/build-project-executor
RUN chmod 0755 /usr/local/bin/build-project-executor

LABEL org.ntwrkfx.profile="project-executor-build/v3" \
      org.ntwrkfx.os="debian-bookworm" \
      org.ntwrkfx.rust="1.96"

ENTRYPOINT ["/usr/local/bin/build-project-executor"]
