# Worker image. Expects prebuilt binaries at dist/<os>/<arch>/{worker,factory}: scripts/worker-image.sh stages them
# from a local release build, the release workflow from the release tarballs.
FROM node:22-bookworm-slim
ARG TARGETOS TARGETARCH
# python3, file and xxd: everyday tools agents reach for (scripted edits, checking downloads, hex dumps).
RUN apt-get update && apt-get install -y --no-install-recommends git curl ca-certificates python3 file xxd \
 && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
 && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" > /etc/apt/sources.list.d/github-cli.list \
 && apt-get update && apt-get install -y --no-install-recommends gh \
 && rm -rf /var/lib/apt/lists/* \
 && npm install -g @anthropic-ai/claude-code
COPY dist/${TARGETOS}/${TARGETARCH}/worker dist/${TARGETOS}/${TARGETARCH}/factory /usr/local/bin/
COPY --chown=node:node skills/factory /home/node/.claude/skills/factory
RUN mkdir /workspace && chown node:node /workspace
USER node
WORKDIR /workspace
ENV FACTORY_AGENT=claude-code FACTORY_WORKSPACE=/workspace
ENTRYPOINT ["worker"]
