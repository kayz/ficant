FROM python@sha256:519591d6871b7bc437060736b9f7456b8731f1499a57e22e6c285135ae657bf7 AS uv-toolchain

ARG UV_URL=https://github.com/astral-sh/uv/releases/download/0.7.13/uv-x86_64-unknown-linux-gnu.tar.gz
ARG UV_SHA256=909278eb197c5ed0e9b5f16317d1255270d1f9ea4196e7179ce934d48c4c2545

RUN set -eu; \
    for attempt in 1 2 3 4 5; do \
      python -c "import urllib.request; urllib.request.urlretrieve('${UV_URL}', '/tmp/uv.tar.gz')" && break; \
      test "${attempt}" -lt 5; \
      sleep "${attempt}"; \
    done; \
    echo "${UV_SHA256}  /tmp/uv.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/uv.tar.gz -C /tmp; \
    install -m 0755 /tmp/uv-x86_64-unknown-linux-gnu/uv /usr/local/bin/uv; \
    install -m 0755 /tmp/uv-x86_64-unknown-linux-gnu/uvx /usr/local/bin/uvx; \
    rm -rf /tmp/uv*

FROM uv-toolchain AS node-runtime

WORKDIR /opt/ficant/python
COPY python/pyproject.toml python/uv.lock ./
RUN uv sync --frozen --no-dev --no-install-project \
    && python --version \
    && uv --version \
    && .venv/bin/python -c "import sys; assert sys.version_info[:3] == (3, 12, 11)"

CMD ["/opt/ficant/python/.venv/bin/python", "-c", "import sys; assert sys.version_info[:3] == (3, 12, 11)"]
