#!/usr/bin/env bash
# Releases the version in plugin.toml:
#   scripts/release.sh ~/fsantomero.key
#
# 1. checks the tree is clean and pushed, runs the tests;
# 2. tags v<version> and pushes the tag: the Build workflow compiles the
#    component on GitHub;
# 3. downloads that component into dist/, packs and signs it;
# 4. creates the GitHub release with the package: the Build workflow runs
#    again on it and checks the package carries exactly its build.
# Prints the package's SHA-256 for publishing/plugins/*.toml.
set -euo pipefail

key="${1:?usage: scripts/release.sh <publisher key file>}"
publisher="fsantomero"
repo="Santom24/openidea-maven-helper"
cd "$(dirname "$0")/.."

# openidea-plugin lives in ~/.cargo/bin, which isn't always on PATH; the
# toolchain is the one pinned in rust-toolchain.toml.
export PATH="$HOME/.cargo/bin:$PATH"
toolchain=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)
rustup toolchain install "$toolchain" --profile minimal --target wasm32-wasip2 >/dev/null
export PATH="$(dirname "$(rustup which cargo --toolchain "$toolchain")"):$PATH"

id=$(sed -n 's/^id = "\(.*\)"/\1/p' plugin.toml | head -1)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' plugin.toml | head -1)
cargo_version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
tag="v$version"
package="$id-$version.oiplugin"
[ "$version" = "$cargo_version" ] || { echo "plugin.toml $version != Cargo.toml $cargo_version" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "commit your changes first" >&2; exit 1; }
git fetch -q origin
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || { echo "HEAD is not origin/main: push first" >&2; exit 1; }
if gh release view "$tag" -R "$repo" >/dev/null 2>&1; then echo "release $tag exists" >&2; exit 1; fi

cargo test --quiet --locked

# The tag (pushing it starts the Build workflow on GitHub).
if ! git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  git tag -a "$tag" -m "$tag"
fi
[ "$(git rev-list -n1 "$tag")" = "$(git rev-parse HEAD)" ] || { echo "$tag is not HEAD" >&2; exit 1; }
git push -q origin "$tag"

echo "waiting for the Build workflow on $tag…"
commit=$(git rev-parse HEAD)
run=""
for _ in $(seq 60); do
  run=$(gh run list -R "$repo" -w build.yml -e push -c "$commit" --json databaseId,headBranch \
        --jq "[.[] | select(.headBranch == \"$tag\")][0].databaseId // empty")
  [ -n "$run" ] && break
  sleep 5
done
[ -n "$run" ] || { echo "no Build run for $tag" >&2; exit 1; }
gh run watch "$run" -R "$repo" --exit-status >/dev/null
echo "build: https://github.com/$repo/actions/runs/$run"

# Its component, packed and signed here.
tmp=$(mktemp -d)
gh run download "$run" -R "$repo" -n "plugin-wasm-$tag" -D "$tmp"
mkdir -p dist
cp "$tmp/plugin.wasm" dist/plugin.wasm
rm -r "$tmp"
openidea-plugin pack . --key "$key" --publisher "$publisher"
openidea-plugin check "$package"

notes=$(sed -n 's/^change-notes = "\(.*\)"/\1/p' plugin.toml | head -1)
gh release create "$tag" "$package" -R "$repo" --title "$tag" \
  --notes "$notes

The component in this package is the one the Build workflow compiled from this tag: https://github.com/$repo/actions/runs/$run"

echo
echo "package:   $package"
echo "sha256:    $(shasum -a 256 "$package" | cut -d' ' -f1)"
echo "component: $(shasum -a 256 dist/plugin.wasm | cut -d' ' -f1) (Build run $run)"
