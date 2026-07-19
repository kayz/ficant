ARG CEPH_IMAGE=quay.io/ceph/ceph@sha256:6b4b5ae33acd3d736eb26d2a19238bce71a22f9cfb99cca887ba6312d0957644
FROM ${CEPH_IMAGE}

USER 0:0
RUN rm -rf /etc/ceph \
    && mkdir -p /var/lib/ceph/etc /var/lib/ceph/mon /var/lib/ceph/osd /var/lib/ceph/radosgw \
    && ln -s /var/lib/ceph/etc /etc/ceph \
    && chown -R 167:167 /var/lib/ceph \
    && chmod 0700 /var/lib/ceph /var/lib/ceph/etc

COPY --chown=167:167 --chmod=0555 deploy/dev/ceph-entrypoint.sh /usr/local/bin/ficant-ceph-entrypoint

LABEL org.opencontainers.image.base.name="quay.io/ceph/ceph@sha256:6b4b5ae33acd3d736eb26d2a19238bce71a22f9cfb99cca887ba6312d0957644" \
      org.opencontainers.image.licenses="LGPL-2.1-only OR LGPL-3.0-only" \
      org.opencontainers.image.title="ficant-ceph-rgw-runtime"

USER 167:167
ENTRYPOINT ["/usr/local/bin/ficant-ceph-entrypoint"]
