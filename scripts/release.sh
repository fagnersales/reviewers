#!/bin/sh
# Builds release binaries and the manifest `install.sh` and `reviewers upgrade` read.
#
#   scripts/release.sh                      # this machine's build only
#   TARGETS="aarch64-apple-darwin x86_64-apple-darwin" scripts/release.sh
#
# Linux targets need cargo-zigbuild (`cargo install cargo-zigbuild`; zig is the linker).
# Output goes to dist/<version>/; upload that folder and dist/latest.txt to BASE_URL.
set -eu

cd "$(dirname "$0")/.."
version=$(awk -F'"' '/^version/ { print $2; exit }' Cargo.toml)
base_url="${BASE_URL:-https://reviewers.sh/releases}"
targets="${TARGETS:-$(rustc -vV | awk '/^host/ { print $2 }')}"
out="dist/$version"
mkdir -p "$out"

manifest="dist/latest.txt"
printf 'version %s\n' "$version" >"$manifest"
if [ -n "${NOTES:-}" ]; then
  printf '%s\n' "$NOTES" | while IFS= read -r note; do printf 'note %s\n' "$note" >>"$manifest"; done
fi

for target in $targets; do
  case "$target" in
    *linux*) builder="cargo zigbuild" ;;
    *) builder="cargo build" ;;
  esac
  rustup target add "$target" >/dev/null 2>&1 || true
  $builder --release --target "$target"
  cp "target/$target/release/reviewers" "$out/reviewers-$target"
  if command -v shasum >/dev/null 2>&1; then
    sha=$(shasum -a 256 "$out/reviewers-$target" | awk '{ print $1 }')
  else
    sha=$(sha256sum "$out/reviewers-$target" | awk '{ print $1 }')
  fi
  printf '%s %s/%s/reviewers-%s %s\n' "$target" "$base_url" "$version" "$target" "$sha" >>"$manifest"
  echo "built $target"
done
cp "$manifest" "$out/latest.txt"
echo "dist/latest.txt:"
cat "$manifest"
