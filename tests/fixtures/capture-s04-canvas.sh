#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "$0")/../.." && pwd)"
cli="${PWCLI_BIN:-playwright-cli}"
if [[ "$cli" == *.sh ]]; then cli_command=(bash "$cli"); else cli_command=("$cli"); fi
port="${BASELINE_PORT:-8766}"
cd "$project_dir"
mkdir -p tests/fixtures/reference/model-s04
if [[ -f tests/fixtures/reference/model-s04/canvas.json && "${S04_REFRESH:-0}" != 1 ]]; then
  echo 'S04 Canvas reference already exists; set S04_REFRESH=1 only after reviewing the source change' >&2
  exit 1
fi

python3 -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1 &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true' EXIT
sleep 1

"${cli_command[@]}" --session clearframe-s04-canvas open "http://127.0.0.1:$port/tests/fixtures/" >/dev/null
"${cli_command[@]}" --raw --session clearframe-s04-canvas run-code --filename tests/fixtures/capture-s04-canvas.js \
  > tests/fixtures/reference/model-s04/canvas.json
"${cli_command[@]}" --session clearframe-s04-canvas close >/dev/null
