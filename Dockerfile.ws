FROM debian:stable-slim

RUN useradd -m -s /bin/bash -u 1000 agent && \
    mkdir -p /etc/openab && \
    chown -R agent:agent /home/agent /etc/openab

ENV HOME=/home/agent
WORKDIR /home/agent

COPY --chown=agent:agent target/release/openab /usr/local/bin/openab
COPY --chown=agent:agent config.toml /etc/openab/config.toml

USER agent

CMD ["openab", "run", "-c", "/etc/openab/config.toml"]
