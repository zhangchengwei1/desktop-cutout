#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "$0")/../.." && pwd)"
cli="${PWCLI_BIN:-playwright-cli}"
if [[ "$cli" == *.sh ]]; then cli_command=(bash "$cli"); else cli_command=("$cli"); fi
port="${BASELINE_PORT:-8766}"
cd "$project_dir"

python3 -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1 &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true' EXIT
sleep 1

"${cli_command[@]}" --session clearframe-model-render open "http://127.0.0.1:$port/tests/fixtures/" >/dev/null
"${cli_command[@]}" --raw --session clearframe-model-render run-code --filename tests/fixtures/capture-model-render.js \
  > tests/fixtures/reference/model-isnet/browser-render-part.json
"${cli_command[@]}" --session clearframe-model-render close >/dev/null
python3 tests/fixtures/verify.py --assemble-model-render
