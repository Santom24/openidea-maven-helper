#!/usr/bin/env bash
# Builds, tests, signs and packs the plugin for a release.
#   scripts/release.sh ~/fsantomero.key
# Prints the package path and its SHA-256 for publishing/plugins/*.toml.
set -euo pipefail

key="${1:?usage: scripts/release.sh <publisher key file>}"
publisher="fsantomero"
cd "$(dirname "$0")/.."

# openidea-plugin lives in ~/.cargo/bin, which isn't always on PATH.
export PATH="$HOME/.cargo/bin:$PATH"

# Reproducible build: the toolchain pinned in rust-toolchain.toml, and the
# cargo home (whose path ends up in panic messages) mapped to /cargo, as the
# Build workflow does. Anyone can rebuild the release and get the same wasm.
toolchain=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)
rustup toolchain install "$toolchain" --profile minimal --target wasm32-wasip2 >/dev/null
export PATH="$(dirname "$(rustup which cargo --toolchain "$toolchain")"):$PATH"
export RUSTFLAGS="--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo"

id=$(sed -n 's/^id = "\(.*\)"/\1/p' plugin.toml | head -1)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' plugin.toml | head -1)
cargo_version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
if [ "$version" != "$cargo_version" ]; then
  echo "plugin.toml version $version differs from Cargo.toml $cargo_version" >&2
  exit 1
fi

cargo test --quiet
openidea-plugin build
echo "wasm sha256: $(shasum -a 256 dist/plugin.wasm | cut -d' ' -f1)"
openidea-plugin pack . --key "$key" --publisher "$publisher"

package="$id-$version.oiplugin"
openidea-plugin check "$package"
echo
echo "package: $package"
echo "sha256:  $(shasum -a 256 "$package" | cut -d' ' -f1)"
echo "tag:     v$version  (attach $package to the GitHub release)"
