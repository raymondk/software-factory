FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target \
    cargo build --release -p worker -p factory && cp target/release/worker target/release/factory /usr/local/bin/

FROM node:22-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends git curl ca-certificates \
 && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
 && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" > /etc/apt/sources.list.d/github-cli.list \
 && apt-get update && apt-get install -y --no-install-recommends gh \
 && rm -rf /var/lib/apt/lists/* \
 && npm install -g @anthropic-ai/claude-code
COPY --from=build /usr/local/bin/worker /usr/local/bin/factory /usr/local/bin/
COPY --chown=node:node skills/factory /home/node/.claude/skills/factory
RUN mkdir /workspace && chown node:node /workspace
USER node
WORKDIR /workspace
ENV FACTORY_AGENT=claude-code FACTORY_WORKSPACE=/workspace
ENTRYPOINT ["worker"]
