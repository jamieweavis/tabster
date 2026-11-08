# Tabster — run `just` to list recipes.

config := env("CONFIG", "debug")
# "-" signs ad hoc.
sign_identity := env("TABSTER_SIGN_IDENTITY", "Apple Development")
bundle_id := "com.jamieweavis.tabster"

rust_flag := if config == "release" { "--release" } else { "" }
rust_out := justfile_directory() / "target" / config
ffi_dir := "shell/Sources/TabsterCoreFFI/include"
generated_dir := "shell/Sources/Tabster/Generated"
app := "build/Tabster.app"

default:
    @just --list

# Build the Rust core static library
core:
    cargo build -p tabster {{rust_flag}}

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
    swift build --package-path shell -c {{config}} -Xlinker -L{{rust_out}}

# Assemble and sign build/Tabster.app
app: build
    rm -rf {{app}}
    mkdir -p {{app}}/Contents/MacOS {{app}}/Contents/Resources
    cp Resources/Info.plist {{app}}/Contents/
    cp -R Resources/Bundle/. {{app}}/Contents/Resources/
    /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $({{just_executable()}} version)" \
        -c "Set :CFBundleVersion $({{just_executable()}} version)" {{app}}/Contents/Info.plist
    cp "$(swift build --package-path shell -c {{config}} --show-bin-path)/Tabster" {{app}}/Contents/MacOS/
    codesign --force --options runtime --entitlements Resources/Tabster.entitlements \
        --sign "{{sign_identity}}" {{app}}

# Package build/Tabster.app as build/Tabster-<version>-macos-arm64.dmg (CONFIG=release to ship)
dmg: app
    #!/usr/bin/env bash
    set -euo pipefail
    dmg="build/Tabster-$({{just_executable()}} version)-macos-arm64.dmg"
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

# Run all tests
test:
    cargo test --workspace

# Run the Swift shell's unit tests (TabsterKit). Builds the whole package, so it needs the core.
test-swift: bindings
    swift test --package-path shell -Xlinker -L{{rust_out}}

# Format Rust code
fmt:
    cargo fmt --all

# Check formatting and run clippy
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

# Run everything CI runs
check: lint test test-swift app

clean:
    cargo clean
    rm -rf build shell/.build {{generated_dir}} {{ffi_dir}}
