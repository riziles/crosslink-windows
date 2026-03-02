FROM ubuntu:24.04

# Core tooling (python3 required for crosslink hooks)
RUN apt-get update && apt-get install -y --no-install-recommends \
    git openssh-client curl ca-certificates jq python3 sudo \
    && rm -rf /var/lib/apt/lists/*

# Non-root agent user with sudo for toolchain installs
RUN useradd -m -s /bin/bash agent && echo "agent ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers.d/agent
USER agent
WORKDIR /home/agent

# Claude CLI
RUN curl -fsSL https://claude.ai/install.sh | sh

# Crosslink binary
COPY --chown=agent crosslink /usr/local/bin/crosslink
COPY --chown=agent entrypoint.sh /usr/local/bin/crosslink-entrypoint.sh
RUN chmod +x /usr/local/bin/crosslink-entrypoint.sh

WORKDIR /workspaces
ENTRYPOINT ["/usr/local/bin/crosslink-entrypoint.sh"]
