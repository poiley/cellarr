# Runtime image for cellarr.
#
# The static musl binaries are cross-compiled by CI (.github/workflows/release.yml)
# and copied in here, so this stays a tiny, build-tool-free runtime layer. The web
# UI is embedded in the binary (rust-embed) and SQLite is bundled, so there are no
# extra assets or services to ship. Built multi-arch (amd64 + arm64) so it runs on
# any node of a mixed cluster without scheduling constraints.
FROM alpine:3.20

# ca-certificates: outbound HTTPS to TheTVDB/TMDb/indexers. tzdata: correct air
# dates / calendar in the operator's timezone.
RUN apk add --no-cache ca-certificates tzdata

# CI drops a per-arch binary (cellarr-amd64 / cellarr-arm64); buildx sets TARGETARCH.
ARG TARGETARCH
COPY cellarr-${TARGETARCH} /usr/local/bin/cellarr

# Bind to all interfaces inside the container (the pod IP is private); persist the
# SQLite DB + config under /data (mount a PersistentVolume there).
ENV CELLARR_API__BIND=0.0.0.0 \
    CELLARR_API__PORT=9494 \
    CELLARR_DATA_DIR=/data

EXPOSE 9494
VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/cellarr"]
CMD ["run"]
