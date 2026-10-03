#!/bin/sh
# Releases the version in Cargo.toml: tags it with its notes and pushes the tag. CI builds every
# binary and publishes them with `latest.txt` as a GitHub Release (.github/workflows/release.yml);
# reviewers.sh/releases/latest.txt redirects there, so installs and upgrades pick it up.
#
#   scripts/release.sh "Reviews run faster" "New: reviewers doctor"    # one argument per release note
set -eu

cd "$(dirname "$0")/.."
version=$(awk -F'"' '/^version/ { print $2; exit }' Cargo.toml)
tag="v$version"
[ -z "$(git status --porcelain)" ] || { echo "commit or stash your changes first" >&2; exit 1; }
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  echo "$tag already exists; bump the version in Cargo.toml first" >&2
  exit 1
fi
[ "$#" -gt 0 ] || { echo "give at least one release note" >&2; exit 1; }
# The site's docs and version come from the binary being released, so they ship with it.
scripts/site.sh
if [ -n "$(git status --porcelain site)" ]; then
  git add site
  git commit -q -m "Site: docs and version for $version"
fi
git push -q origin HEAD
git tag -a "$tag" -m "$(printf '%s\n' "$@")"
git push -q origin "$tag"
echo "Pushed $tag. Follow the build: gh run watch"
