#!/usr/bin/env bash
# Runs the spike in every loading mode and writes one JSONL per run to results/.
# Usage: spikes/youtube/run-matrix.sh [name-filter]   (opens a window per run, ~5 minutes total)
set -uo pipefail
cd "$(dirname "$0")"
swift build >/dev/null || exit 1
bin="$(swift build --show-bin-path)/YouTubeSpike"
mkdir -p results
FILTER="${1-}"

ok=dQw4w9WgXcQ          # embeddable music video
noembed=gUYU3FqMLT8     # owner disabled embedding (youtube-dl issue #8403)
bogus=xxxxxxxxxxx       # does not exist

run() {
    local name=$1; shift
    [[ -n "$FILTER" && "$name" != *"$FILTER"* ]] && return 0
    "$bin" "$@" >"results/$name.jsonl" 2>/dev/null
    printf '%-40s ' "$name"
    tail -1 "results/$name.jsonl" | python3 -c 'import json,sys; r=json.load(sys.stdin); print(r.get("verdict"), "errors=%s" % r.get("errors"), " ".join(r.get("events", [])))'
}

# Full scripted runs (seek / rate / pause / play), 20 s
run html-baseurl-tabster.local --mode html-baseurl --base https://tabster.local/ --video $ok --seconds 20
run localhost                   --mode localhost --video $ok --seconds 20
run localhost-127.0.0.1         --mode localhost --server-host 127.0.0.1 --video $ok --seconds 10 --no-script

# Loading-mode variants, 10 s, no command script
short=(--video $ok --seconds 10 --no-script)
run html-nobase                 --mode html-nobase "${short[@]}"
run html-baseurl-nocookie-base  --mode html-baseurl --base https://www.youtube-nocookie.com/ "${short[@]}"
run html-baseurl-http-localhost --mode html-baseurl --base http://localhost/ "${short[@]}"
run html-baseurl-http-127.0.0.1 --mode html-baseurl --base http://127.0.0.1/ "${short[@]}"
run html-baseurl-https-127.0.0.1 --mode html-baseurl --base https://127.0.0.1/ "${short[@]}"
run file                        --mode file "${short[@]}"
run file-safari-ua              --mode file --ua safari "${short[@]}"
run html-nobase-safari-ua       --mode html-nobase --ua safari "${short[@]}"
run html-baseurl-host-nocookie  --mode html-baseurl --host nocookie "${short[@]}"
run localhost-host-nocookie     --mode localhost --host nocookie "${short[@]}"
run html-baseurl-iframe-api     --mode html-baseurl --iframe api "${short[@]}"
run localhost-iframe-api        --mode localhost --iframe api "${short[@]}"
run html-baseurl-safari-ua      --mode html-baseurl --ua safari "${short[@]}"

# Error codes
run noembed-html-baseurl        --mode html-baseurl --video $noembed --seconds 8 --no-script
run noembed-localhost           --mode localhost --video $noembed --seconds 8 --no-script
run bogus-id                    --mode html-baseurl --video $bogus --seconds 8 --no-script

# The user's real target video (Men I Trust, "Show Me How")
target=cJRFMfztgBg
run target-html-baseurl         --mode html-baseurl --video $target --seconds 20
run target-localhost            --mode localhost --video $target --seconds 20
run target-file                 --mode file --video $target --seconds 8 --no-script

# Reporter at setInterval(100) instead of rAF
run localhost-interval100       --mode localhost --video $ok --seconds 20 --reporter 100
