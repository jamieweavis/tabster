# Tabster — run `just` to list recipes.

config := env("CONFIG", "debug")
# "-" signs ad hoc.
sign_identity := env("TABSTER_SIGN_IDENTITY", "Apple Development")
bundle_id := "com.jamieweavis.tabster"

rust_flag := if config == "release" { "--release" } else { "" }
# Release builds are universal (arm64 + x86_64); debug builds are native only. UNIVERSAL=0/1 overrides.
universal := env("UNIVERSAL", if config == "release" { "1" } else { "0" })
# Where the bindings are generated from (the arm64 slice when universal: both slices bind identically).
rust_out := if universal == "1" { justfile_directory() / "target" / "aarch64-apple-darwin" / config } else { justfile_directory() / "target" / config }
# Where the shell links libtabster_core.a from (lipo'd fat archive when universal).
lib_dir := if universal == "1" { justfile_directory() / "build" / "universal" / config } else { rust_out }
swift_arch := if universal == "1" { "--arch arm64 --arch x86_64" } else { "" }
dmg_arch := if universal == "1" { "universal" } else if arch() == "aarch64" { "arm64" } else { "x86_64" }
ffi_dir := "shell/Sources/TabsterCoreFFI/include"
generated_dir := "shell/Sources/Tabster/Generated"
app := "build/Tabster.app"

default:
    @just --list

# Build the Rust core static library (both architectures, lipo'd together, when universal)
core:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "{{universal}}" != 1 ]; then
        cargo build -p tabster {{rust_flag}}
        exit 0
    fi
    for t in aarch64-apple-darwin x86_64-apple-darwin; do
        rustup target list --installed | grep -qx "$t" || { echo "missing Rust target: rustup target add $t" >&2; exit 1; }
    done
    cargo build -p tabster {{rust_flag}} --target aarch64-apple-darwin --target x86_64-apple-darwin
    mkdir -p "{{lib_dir}}"
    lipo -create -output "{{lib_dir}}/libtabster_core.a" \
        target/aarch64-apple-darwin/{{config}}/libtabster_core.a target/x86_64-apple-darwin/{{config}}/libtabster_core.a

# Regenerate the Swift bindings for the Rust core
bindings: core
    mkdir -p {{ffi_dir}} {{generated_dir}}
    cargo run -q -p uniffi-bindgen -- generate --library {{rust_out}}/libtabster_core.a \
        --language swift --out-dir build/bindings
    mv build/bindings/tabster_coreFFI.h {{ffi_dir}}/
    mv build/bindings/tabster_coreFFI.modulemap {{ffi_dir}}/module.modulemap
    mv build/bindings/tabster_core.swift {{generated_dir}}/

# Build the Swift shell, linked against the Rust core
build: bindings
    swift build --package-path shell -c {{config}} {{swift_arch}} -Xlinker -L{{lib_dir}}

# Assemble and sign build/Tabster.app
app: build soundfont
    rm -rf {{app}}
    mkdir -p {{app}}/Contents/MacOS {{app}}/Contents/Resources
    cp Resources/Info.plist {{app}}/Contents/
    cp -R Resources/Bundle/. {{app}}/Contents/Resources/
    /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $({{just_executable()}} version)" \
        -c "Set :CFBundleVersion $({{just_executable()}} version)" {{app}}/Contents/Info.plist
    cp "$(swift build --package-path shell -c {{config}} {{swift_arch}} --show-bin-path)/Tabster" {{app}}/Contents/MacOS/
    codesign --force --options runtime --entitlements Resources/Tabster.entitlements \
        --sign "{{sign_identity}}" {{app}}

# Package build/Tabster.app as build/Tabster-<version>-macos-<arch>.dmg (CONFIG=release to ship a universal one)
dmg: app
    #!/usr/bin/env bash
    set -euo pipefail
    dmg="build/Tabster-$({{just_executable()}} version)-macos-{{dmg_arch}}.dmg"
    rm -rf build/dmg
    mkdir -p build/dmg
    cp -R {{app}} build/dmg/
    ln -s /Applications build/dmg/Applications
    hdiutil create -volname Tabster -srcfolder build/dmg -format UDZO -ov "$dmg"
    (cd build && shasum -a 256 "$(basename "$dmg")" > "$(basename "$dmg").sha256")

# Print the app's version (the workspace version, which release-plz bumps)
version:
    @cargo pkgid -p tabster | sed 's/.*[#@]//'

# Build, then (re)launch the app
run: app
    -pkill -x Tabster
    # Wait for LaunchServices, not just the kernel, to see the old instance gone, or open fails with -600.
    @while [ -n "$(lsappinfo find bundleid={{bundle_id}})" ]; do sleep 0.1; done
    open {{app}}

# Stream the app's logs; `just logs info` hides the debug ones
logs level="debug":
    log stream --style compact --level {{level}} --predicate 'subsystem == "{{bundle_id}}"'

# Show the last `period` of the app's logs (debug ones aren't kept)
logs-recent period="1h":
    log show --style compact --info --last {{period}} --predicate 'subsystem == "{{bundle_id}}"'

# Download the bundled GM SoundFont (GeneralUser GS 2.0.3, ~32 MB) into Resources/Bundle/
soundfont:
    #!/usr/bin/env bash
    set -euo pipefail
    out="Resources/Bundle/SoundFont.sf2"
    sha="9575028c7a1f589f5770fccc8cff2734566af40cd26ed836944e9a5152688cfe"
    if [ -f "$out" ] && echo "$sha  $out" | shasum -a 256 -c --status; then
        echo "$out is up to date"; exit 0
    fi
    url="https://raw.githubusercontent.com/mrbumpy409/GeneralUser-GS/684543d5e5efaef08d02be50dcda8d552478fa60/GeneralUser-GS.sf2"
    curl -fL --retry 3 -o "$out.part" "$url"
    echo "$sha  $out.part" | shasum -a 256 -c
    mv "$out.part" "$out"

# Run all tests
test:
    cargo test --workspace

# Run the Swift shell's unit tests (TabsterKit). Builds the whole package, so it needs the core.
test-swift: bindings
    swift test --package-path shell -Xlinker -L{{lib_dir}}

# Format Rust code
fmt:
    cargo fmt --all

# Check formatting and run clippy
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

# Run everything CI runs
check: lint test test-swift app

# Fuzz a target for `secs` seconds (parse_any, gpif_xml, gpx_container, gp5_binary, pipeline).
# Needs nightly (`rustup toolchain install nightly --profile minimal`) and `cargo install cargo-fuzz`.
# Crashes land in fuzz/artifacts/<target>/; keep minimised ones in testdata/regressions/<target>/.
fuzz target="parse_any" secs="60" jobs="1":
    #!/usr/bin/env bash
    set -euo pipefail
    ./fuzz/seed.sh
    export PATH="$(dirname "$(rustup which --toolchain nightly cargo)"):$PATH"
    cd fuzz
    set --
    case "{{target}}" in gpif_xml|pipeline) set -- "-dict=$PWD/gpif.dict" ;; esac
    cargo fuzz run -O -a "{{target}}" -- "$@" -fork="{{jobs}}" -max_total_time="{{secs}}" \
        -timeout=5 -rss_limit_mb=2048 -max_len=65536

clean:
    cargo clean
    rm -rf build shell/.build {{generated_dir}} {{ffi_dir}}
