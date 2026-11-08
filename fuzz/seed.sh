#!/usr/bin/env bash
# Seed fuzz/corpus/<target>/ from testdata/ (and the regression inputs). Idempotent.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
corpus="$root/fuzz/corpus"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# .gp seeds: each GPIF fixture zipped as Content/score.gpif.
for gpif in "$root"/testdata/gp7/*.gpif "$root"/testdata/gpx/*.gpif; do
    name="$(basename "$gpif" .gpif)"
    mkdir -p "$tmp/$name/Content"
    cp "$gpif" "$tmp/$name/Content/score.gpif"
    (cd "$tmp/$name" && zip -q -X "$tmp/$name.gp" Content/score.gpif)
done

seed() { # target files...
    local target="$1"
    shift
    mkdir -p "$corpus/$target"
    cp "$@" "$corpus/$target/"
    if [ -d "$root/testdata/regressions/$target" ]; then
        cp "$root/testdata/regressions/$target"/* "$corpus/$target/" 2>/dev/null || true
    fi
}

gp5=("$root"/testdata/gp5/*.gp5)
gpx=("$root"/testdata/gpx/*.gpx)
gpif=("$root"/testdata/gp7/*.gpif "$root"/testdata/gpx/*.gpif)
gp=("$tmp"/*.gp)

seed parse_any "${gp5[@]}" "${gpx[@]}" "${gp[@]}"
seed gpif_xml "${gpif[@]}"
seed gpx_container "${gpx[@]}"
seed gp5_binary "${gp5[@]}"
seed pipeline "${gp5[@]}" "${gpx[@]}" "${gp[@]}" "${gpif[@]}"
