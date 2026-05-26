FROM authzed/zed:latest AS zedbin

FROM debian:bookworm-slim

COPY --from=zedbin /usr/local/bin/zed /usr/local/bin/zed
COPY devops/docker/compose/services/spicedb/schema.zed /opt/a2a/spicedb/schema.zed
COPY devops/docker/compose/services/spicedb/relationships.yaml /opt/a2a/spicedb/relationships.yaml
COPY devops/docker/compose/services/a2a-spicedb-seed/entrypoint.sh /entrypoint.sh

RUN chmod +x /entrypoint.sh \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ENTRYPOINT ["/entrypoint.sh"]
