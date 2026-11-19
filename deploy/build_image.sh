#!/usr/bin/env bash
# Builds the release binary under a RAM/CPU cap and packs it into an image
# tagged with the commit, so every deploy can be rolled back by tag.
set -euo pipefail
cd "$(dirname "$0")/.."
commit=$(git rev-parse --short HEAD)
/opt/projects/tools/bin/run-limited --mem 3G -- cargo build --release --bin aion_agent_conversation_store
docker build -f deploy/Dockerfile -t "aion_agent_conversation_store:${commit}" -t aion_agent_conversation_store:current .
echo "built aion_agent_conversation_store:${commit}"
