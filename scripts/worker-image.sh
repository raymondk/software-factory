#!/usr/bin/env bash
# Builds the worker image from a local build. Usage: scripts/worker-image.sh [tag]
# worker and factory are compiled in a rust:1-bookworm container so they link against the image's glibc.
set -euo pipefail
cd "$(dirname "$0")/.."
tag=${1:-software-factory/worker:latest}
case "$(uname -m)" in x86_64) arch=amd64 ;; aarch64|arm64) arch=arm64 ;; *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;; esac
mkdir -p target/bookworm
docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp -e CARGO_HOME=/src/target/bookworm/cargo -v "$PWD:/src" -w /src rust:1-bookworm \
  cargo build --release --target-dir target/bookworm -p worker -p factory
mkdir -p "dist/linux/$arch"
cp target/bookworm/release/worker target/bookworm/release/factory "dist/linux/$arch/"
docker build -t "$tag" .
