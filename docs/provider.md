# Running a provider

A provider runs where the workers run. It holds the credentials the workers need, starts and stops worker containers
when the orchestrator asks, and answers `GET /status`. The orchestrator never sees the git or agent tokens. A provider
belongs to the developer who registers it: its workers act as that developer and work only their tickets.

## Prerequisites

- **Docker**, with the daemon reachable by the user who runs the provider (`docker ps` works without `sudo`).
- **The `docker-provider` binary** from a [release](https://github.com/raymondk/software-factory/releases), or `cargo build --release -p docker-provider`.
- **Network, both ways.** The orchestrator must reach the provider's `listen` address. The workers must reach the
  orchestrator at its `public_url` (`[orchestrator]` in `factory.toml`); `localhost` there works only when both run on
  the same Docker host.
- **A git token** with push access to the project's repos, and permission to open pull requests. The worker feeds it to
  `git` and `gh` as `GIT_TOKEN`.
- **An agent credential.** For Claude Code, `CLAUDE_CODE_OAUTH_TOKEN`: run `claude setup-token` on a machine where you
  are signed in to Claude Code and copy the token it prints.
- **An account on the orchestrator.** Sign in to the UI with Internet Identity; the admin approves you with
  `factory user approve <principal> --name <name>` (the pending page shows the command). Only approved users can add providers.

## Install

Download the tarball for your architecture from the release, check it, and put the binary on the path:

    tar -xzf software-factory-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz docker-provider
    sha256sum -c --ignore-missing SHA256SUMS
    install -m 0755 docker-provider ~/.local/bin/

Pull the worker image of the same version. `worker` is the base image; `worker-icp` adds the ICP toolchain (Rust with
`wasm32`, Motoko, mops, `icp-cli`) for canister projects:

    docker pull ghcr.io/raymondk/software-factory/worker:vX.Y.Z        # or worker-icp:vX.Y.Z

Keep the provider, the image and the orchestrator on the same release: a worker that cannot decode the orchestrator's
responses exits at once, and the Providers view shows each provider's and worker's version so a stale one stands out.

## Configure

Copy `provider.example.toml` to `provider.toml`:

    [provider]
    listen = "0.0.0.0:8081"      # where the orchestrator reaches this provider
    max_workers = 4              # containers at once
    token = "..."                # invent it (openssl rand -hex 32); you give it to the orchestrator when registering

    [agents.claude-code]
    image = "ghcr.io/raymondk/software-factory/worker:vX.Y.Z"
    models = ["sonnet", "opus"]  # what tickets may ask for
    default_model = "sonnet"     # when a ticket sets none

The secrets go in `provider.secrets.toml` next to it, same layout, merged over it, gitignored:

    [worker_env]
    GIT_TOKEN = "..."
    CLAUDE_CODE_OAUTH_TOKEN = "..."

Everything under `worker_env` becomes environment in every worker container. `agents.<name>` must be an agent the
orchestrator's `factory.toml` lists under `[agents]`, or it is never scheduled.

## Run

    docker-provider provider.toml

A healthy start logs one line, with the agents, the address, the version and `max_workers`:

    INFO docker_provider: docker provider listening version="0.1.0" agents=claude-code addr=0.0.0.0:8081 max_workers=4

Check it answers: `curl http://localhost:8081/status` returns the version, capacity, in-use count, the agents with their
models, and the running workers. `RUST_LOG=debug` adds every request and docker command.

To keep it running, a user unit (`~/.config/systemd/user/docker-provider.service`):

    [Service]
    ExecStart=%h/.local/bin/docker-provider %h/factory/provider.toml
    Restart=on-failure

    [Install]
    WantedBy=default.target

then `systemctl --user enable --now docker-provider` and `journalctl --user -u docker-provider -f`. Workers survive a
provider restart: it finds its containers again by label.

## Register

In the UI, open the cog at the top right and add a provider with a name, the URL the orchestrator should use
(`http://<host>:8081`) and the token from `provider.toml`. Or, with a personal token from the Tokens modal:

    FACTORY_URL=https://factory.example FACTORY_TOKEN=... factory provider add home http://<host>:8081 <provider token>

The Providers view on the board lists every provider. Within a scheduler pass (`scheduler.interval`, default 10s) yours
shows **reachable** with when it last answered, its version, `in use / capacity`, and the agents and models it
advertises. **Unreachable** means the orchestrator's last request failed; hover it for the error.

## Verify

Create a ticket, set its owner to you if the admin created it, and move it to `ready`. Within a scheduler pass a
worker appears in the Workers pane with your provider's id, then goes `busy` with the ticket. Its log opens from the
pane; the agent's output streams there. When the queue empties the scheduler stops the worker.

## Troubleshooting

- **unreachable, `connection refused`**: the provider is not running, or `listen` binds an address the orchestrator
  cannot reach. Check `curl <url>/status` from the orchestrator's host.
- **unreachable, `401`**: the token registered does not match `provider.token`. Edit it under the cog.
- **worker starts, dies at once**: it cannot reach the orchestrator. Look at the worker's log in the Workers pane, or
  `docker logs` the container; `FACTORY_URL` is the orchestrator's `public_url`, which must resolve from inside the
  container (`host.docker.internal`, not `localhost`).
- **`docker run: Unable to find image`**: pull the image, or fix `agents.<name>.image`.
- **`permission denied ... docker.sock`**: add the provider's user to the `docker` group and log in again.
- **agent fails on git push or `gh pr create`**: `GIT_TOKEN` lacks access to the repo, or the repo is not in the
  project's `repos`. The run's log shows the command's error.
- **`agents.x: default_model is not in models`** at startup: fix `provider.toml`; the provider refuses a config it
  cannot honour.

## Updating

Releases move the binaries and the image together. Download the new tarball, `docker pull` the new image tag, change
`image` in `provider.toml`, and restart the provider; running workers finish on the old image. The board shows the
version each provider and worker runs.
