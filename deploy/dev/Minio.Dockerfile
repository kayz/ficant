ARG MINIO_IMAGE=minio/minio@sha256:a1ea29fa28355559ef137d71fc570e508a214ec84ff8083e39bc5428980b015e
FROM ${MINIO_IMAGE}

USER 0:0
RUN mkdir -p /data \
    && chown 1000:1000 /data \
    && chmod 0700 /data

LABEL org.opencontainers.image.base.name="minio/minio@sha256:a1ea29fa28355559ef137d71fc570e508a214ec84ff8083e39bc5428980b015e" \
      org.opencontainers.image.licenses="AGPL-3.0-only" \
      org.opencontainers.image.title="ficant-minio-runtime"

USER 1000:1000
