#!/usr/bin/env bash
# Builds the ICP worker image on top of a base worker image. Usage: scripts/worker-icp-image.sh [tag] [base image]
set -euo pipefail
cd "$(dirname "$0")/.."
docker build -f Dockerfile.worker-icp --build-arg "BASE_IMAGE=${2:-software-factory/worker:latest}" -t "${1:-software-factory/worker-icp:latest}" .
