#!/usr/bin/env bash
# No live UI: both runs use a private recording D-Bus service.
set -euo pipefail
repro_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
repro_dir=$(mktemp -d "${TMPDIR:-/tmp}/computer-use-input-repro.XXXXXX")
repro_base=897b8e025fb77791a0867b21c9fe9f5a0a47ac4b
command -v dbus-daemon >/dev/null || { echo 'Install dbus-daemon (package dbus) first.' >&2; exit 1; }
mkdir "$repro_dir/baseline"
git -C "$repro_root" archive "$repro_base" | tar -x -C "$repro_dir/baseline"
cp "$repro_root/src/platform/linux/input_tests.rs" "$repro_dir/baseline/src/platform/linux/input_tests.rs"
printf '\n#[cfg(test)]\nmod input_tests;\n' >> "$repro_dir/baseline/src/platform/linux/mod.rs"
repro_test=platform::linux::input_tests::background_input_never_emits_desktop_events
printf 'Evidence directory: %s\nBaseline: %s\n' "$repro_dir" "$repro_base"
if cargo test --locked --target-dir "$repro_dir/target-baseline" --manifest-path "$repro_dir/baseline/Cargo.toml" --lib "$repro_test" -- --exact --nocapture > "$repro_dir/baseline.log" 2>&1; then
    echo 'ERROR: the unpatched baseline unexpectedly passed.' >&2
    exit 1
fi
if ! grep -q 'Background input must fail before emitting events. Unexpected results:' "$repro_dir/baseline.log"; then
    cat "$repro_dir/baseline.log"
    echo 'ERROR: baseline failed for an infrastructure/build reason, not the expected regression.' >&2
    exit 1
fi
echo 'Baseline reproduced: background requests emitted desktop-wide input.'
cargo test --locked --target-dir "$repro_dir/target-fixed" --manifest-path "$repro_root/Cargo.toml" --lib platform::linux::input_tests -- --nocapture > "$repro_dir/fixed.log" 2>&1
cat "$repro_dir/fixed.log"
echo 'Fixed version passed: background requests rejected; explicit global input still works.'
