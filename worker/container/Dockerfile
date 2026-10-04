# The jurl.dev playground: the released jurl binary behind a tiny HTTP server.
FROM python:3.13-slim

ARG JURL_VERSION=v0.1.0
ARG JURL_SHA256=b8cbb4022a57641b822e2a1ed2fbb819e2bfa249507daf3fa02125ff6617c036

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl xz-utils \
 && curl -fsSLo /tmp/jurl.tar.xz "https://github.com/rayoplateado/jurl/releases/download/${JURL_VERSION}/jurl-x86_64-unknown-linux-gnu.tar.xz" \
 && echo "${JURL_SHA256}  /tmp/jurl.tar.xz" | sha256sum -c - \
 && tar -xJf /tmp/jurl.tar.xz -C /tmp \
 && install -m 0755 /tmp/jurl-x86_64-unknown-linux-gnu/jurl /usr/local/bin/jurl \
 && rm -rf /tmp/jurl* \
 && apt-get purge -y curl xz-utils && apt-get autoremove -y && rm -rf /var/lib/apt/lists/*

# No JavaScript rendering in the playground: never download Lightpanda.
ENV JURL_NO_DOWNLOAD=1 PYTHONUNBUFFERED=1
RUN useradd --create-home jurl
USER jurl
COPY server.py /app/server.py
EXPOSE 8080
CMD ["python", "/app/server.py"]
