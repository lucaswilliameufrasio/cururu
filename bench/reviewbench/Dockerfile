# ReviewBench adapter image for the released Cururu CLI.
# Build: docker build --platform linux/amd64 -t cururu-reviewbench:dev bench/reviewbench
FROM debian:trixie-slim

ARG CURURU_VERSION=5.1.0
ARG CURURU_SHA256=7ec882964a6fb4bd42e091dcd70b96a049a1c7dacd04e9558860929bd83ee9f8

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git jq xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN curl --fail --location --proto '=https' --tlsv1.2 --connect-timeout 30 --max-time 600 --retry 3 \
        "https://github.com/lucaswilliameufrasio/cururu/releases/download/v${CURURU_VERSION}/cururu-x86_64-unknown-linux-gnu.tar.xz" \
        --output /tmp/cururu.tar.xz \
    && echo "${CURURU_SHA256}  /tmp/cururu.tar.xz" | sha256sum --check - \
    && tar -xJf /tmp/cururu.tar.xz -C /tmp \
    && install /tmp/cururu-x86_64-unknown-linux-gnu/cururu /usr/local/bin/cururu \
    && rm -rf /tmp/cururu*

COPY entrypoint.sh /usr/local/bin/cururu-reviewbench
ENTRYPOINT ["/usr/local/bin/cururu-reviewbench"]
