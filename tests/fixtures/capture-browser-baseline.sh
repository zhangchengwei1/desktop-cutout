#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "$0")/../.." && pwd)"
cli="${PWCLI_BIN:-playwright-cli}"
if [[ "$cli" == *.sh ]]; then cli_command=(bash "$cli"); else cli_command=("$cli"); fi
port="${BASELINE_PORT:-8766}"
cd "$project_dir"
mkdir -p tests/fixtures/reference

python3 -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1 &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true' EXIT
sleep 1

for group in 'a 0 3' 'b 3 5' 'c 5 7'; do
  read -r label start end <<< "$group"
  session="clearframe-baseline-$label"
  "${cli_command[@]}" --session "$session" open "http://127.0.0.1:$port/tests/fixtures/?start=$start&end=$end" >/dev/null
  "${cli_command[@]}" --raw --session "$session" run-code --filename tests/fixtures/capture-browser-baseline.js \
    > "tests/fixtures/reference/browser-canvas-part-$label.json"
  "${cli_command[@]}" --session "$session" close >/dev/null
done

python3 tests/fixtures/verify.py --assemble
