#!/usr/bin/env bash
# Builds, tests, signs and packs the plugin for a release.
#   scripts/release.sh ~/fsantomero.key
# Prints the package path and its SHA-256 for publishing/plugins/*.toml.
set -euo pipefail

key="${1:?usage: scripts/release.sh <publisher key file>}"
publisher="fsantomero"
cd "$(dirname "$0")/.."

id=$(sed -n 's/^id = "\(.*\)"/\1/p' plugin.toml)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' plugin.toml)
cargo_version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
if [ "$version" != "$cargo_version" ]; then
  echo "plugin.toml version $version differs from Cargo.toml $cargo_version" >&2
  exit 1
fi

cargo test --quiet
openidea-plugin build
openidea-plugin pack . --key "$key" --publisher "$publisher"

package="$id-$version.oiplugin"
openidea-plugin check "$package"
echo
echo "package: $package"
echo "sha256:  $(shasum -a 256 "$package" | cut -d' ' -f1)"
echo "tag:     v$version  (attach $package to the GitHub release)"
