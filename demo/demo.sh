#!/usr/bin/env bash
# One ticket end to end through one worker. Usage: demo.sh "title" ["description"]
# DEMO_AGENT=command runs a shell stand-in instead of Claude Code (no tokens needed);
# DEMO_SLEEP (seconds, default 5) is how long the stand-in "works" before finishing.
# DEMO_KILL=1 kills the worker container once the ticket is in_progress: the ticket keeps its state and a new worker resumes it.
# Needs docker, sqlite3 and sha256sum.
set -euo pipefail
cd "$(dirname "$0")/.."
title=${1:?usage: demo.sh "title" ["description"]}
description=${2:-}
agent=${DEMO_AGENT:-claude-code}
if [ "$agent" != command ] && { [ -z "${GIT_TOKEN:-}" ] || [ -z "${CLAUDE_CODE_OAUTH_TOKEN:-}" ]; }; then
  echo "export GIT_TOKEN and CLAUDE_CODE_OAUTH_TOKEN, or run with DEMO_AGENT=command" >&2
  exit 1
fi

cargo build --release -p orchestrator -p docker-provider -p factory
docker build -t software-factory/worker:latest .
export PATH=$PWD/target/release:$PATH

tmp=$(mktemp -d)
echo "Configs and logs in $tmp"
if [ "$agent" = command ]; then
  sleep=${DEMO_SLEEP:-${DEMO_KILL:+30}}
  sleep=${sleep:-5}
  sed -e 's/^heartbeat_timeout = .*/heartbeat_timeout = "10s"/' -e '/^\[agents/,$d' demo/factory.toml > "$tmp/factory.toml"
  cat >> "$tmp/factory.toml" <<TOML
[agents.command]
run_timeout = "30m"

[prompts]
ready = '''
set -e
factory ticket edit {{ticket.id}} --state in_progress > /dev/null
echo "working on #{{ticket.id}} for ${sleep}s"
sleep $sleep
factory ticket comment {{ticket.id}} --body "Stand-in agent: implemented {{ticket.id}}" > /dev/null
factory ticket edit {{ticket.id}} --add-link https://github.com/example/repo/pull/1 > /dev/null
factory ticket edit {{ticket.id}} --state in_review > /dev/null
echo '{"tokens_in":1200,"tokens_out":300,"cost":0.0123}'
'''
in_progress = '''
set -e
factory ticket comment {{ticket.id}} --body "resumed" > /dev/null
factory ticket edit {{ticket.id}} --state in_review > /dev/null
echo '{"tokens_in":100,"tokens_out":20,"cost":0.001}'
'''
TOML
  sed '/^\[agents/,$d' demo/provider.toml > "$tmp/provider.toml"
  cat >> "$tmp/provider.toml" <<'TOML'
[agents.command]
image = "software-factory/worker:latest"
models = ["sh"]
default_model = "sh"

[worker_env]
FACTORY_AGENT_COMMAND = "/bin/sh"
FACTORY_HEARTBEAT_INTERVAL = "2s"
FACTORY_POLL_INTERVAL = "2s"
TOML
else
  cp demo/factory.toml "$tmp/factory.toml"
  sed -e "s|\${GIT_TOKEN}|$GIT_TOKEN|" -e "s|\${CLAUDE_CODE_OAUTH_TOKEN}|$CLAUDE_CODE_OAUTH_TOKEN|" demo/provider.toml > "$tmp/provider.toml"
fi

cleanup() {
  echo "Stopping orchestrator, provider, and worker containers"
  kill "$orch" "$prov" 2> /dev/null || true
  containers=$(docker ps -aq --filter label=software-factory.worker_id)
  [ -z "$containers" ] || docker rm -f "$containers" > /dev/null
  echo "Logs: $tmp/orchestrator.log $tmp/provider.log"
}
docker-provider "$tmp/provider.toml" > "$tmp/provider.log" 2>&1 &
prov=$!
orchestrator "$tmp/factory.toml" > "$tmp/orchestrator.log" 2>&1 &
orch=$!
trap cleanup EXIT
export FACTORY_URL=http://localhost:8080
FACTORY_TOKEN=$(sed -n 's/^token = "\(.*\)"/\1/p' "$tmp/factory.toml" | head -1)
export FACTORY_TOKEN
for _ in $(seq 50); do factory ticket list > /dev/null 2>&1 && break; sleep 0.2; done
factory ticket list > /dev/null

# Providers belong to developers: sign a demo developer in (approved user, personal token straight into the
# database, as the UI login would) and add the Docker provider as them. The rest of the demo runs as that developer.
factory user approve demo-principal --name Demo > /dev/null
demo_token=demo-user-token
sqlite3 "$tmp/factory.db" "INSERT INTO personal_tokens (token_hash, principal, name, created_at) \
  VALUES ('$(printf %s "$demo_token" | sha256sum | cut -d' ' -f1)', 'demo-principal', 'demo', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))"
export FACTORY_TOKEN=$demo_token
provider_token=$(sed -n 's/^token = "\(.*\)"/\1/p' "$tmp/provider.toml" | head -1)
factory provider add local http://localhost:8081 "$provider_token" > /dev/null

id=$(factory ticket create --title "$title" --description "$description" | sed -n 's/^  "id": \([0-9]*\),/\1/p')
factory ticket edit "$id" --state ready > /dev/null
echo "Ticket #$id is ready; waiting for a worker"

# Prints state, assignee, comment and link changes until the ticket leaves ready/in_progress.
prev=""
killed=
while :; do
  cur=$(factory ticket view "$id" | grep -E '^ +("(state|assignee|author|body)": |"https?://)')
  diff <(printf '%s\n' "$prev") <(printf '%s\n' "$cur") | sed -n "s/^> /$(date +%T) /p" || true
  prev=$cur
  state=$(printf '%s\n' "$cur" | sed -n 's/^  "state": "\(.*\)",*/\1/p')
  if [ -n "${DEMO_KILL:-}" ] && [ -z "$killed" ] && [ "$state" = in_progress ]; then
    killed=$(docker ps -q --filter label=software-factory.worker_id)
    docker rm -f $killed > /dev/null
    echo "$(date +%T) killed the worker container; waiting for the reaper and a new worker"
  fi
  case $state in
    ready | in_progress) sleep 2 ;;
    *) break ;;
  esac
done

echo; echo "Metrics:"; factory metrics
echo; echo "Waiting for the scheduler to stop the worker"
for _ in $(seq 60); do
  [ -n "$(factory worker list | awk '$3 != "dead"')" ] || break
  sleep 2
done
echo "Workers:"; factory worker list
echo "Containers:"; docker ps -a --filter label=software-factory.worker_id --format '{{.ID}} {{.Status}}'
