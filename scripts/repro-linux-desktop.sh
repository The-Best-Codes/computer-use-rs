#!/usr/bin/env bash
# Builds the same public API driver against baseline and patched code, then
# runs only in a private authenticated Xvfb display and private D-Bus session.
set -euo pipefail
repro_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
repro_dir=$(mktemp -d "${TMPDIR:-/tmp}/computer-use-desktop-repro.XXXXXX")
for tool in Xvfb dbus-daemon python3; do
    command -v "$tool" >/dev/null || { echo "Missing prerequisite: $tool" >&2; exit 1; }
done
python3 -c 'import gi; gi.require_version("Gtk", "3.0"); from gi.repository import Gtk; from PIL import Image'
mkdir "$repro_dir/baseline"
git -C "$repro_root" archive 897b8e025fb77791a0867b21c9fe9f5a0a47ac4b | tar -x -C "$repro_dir/baseline"
cp "$repro_root/examples/linux_input_destination.rs" "$repro_dir/baseline/examples/"
printf 'Evidence directory: %s\n' "$repro_dir"
cargo build --locked --manifest-path "$repro_dir/baseline/Cargo.toml" --target-dir "$repro_dir/target-baseline" --example linux_input_destination > "$repro_dir/build-baseline.log" 2>&1
cargo build --locked --manifest-path "$repro_root/Cargo.toml" --target-dir "$repro_dir/target-fixed" --example linux_input_destination > "$repro_dir/build-fixed.log" 2>&1
python3 "$repro_root/scripts/linux_input_desktop.py" "$repro_dir" "$repro_dir/target-baseline/debug/examples/linux_input_destination" "$repro_dir/target-fixed/debug/examples/linux_input_destination"
