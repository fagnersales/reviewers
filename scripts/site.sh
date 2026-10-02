#!/bin/sh
# Builds the static site in site/: the installer at /install, the agent guide
# at /docs.md and /llms.txt, and the version and size the homepage shows.
#
#   scripts/site.sh            # then serve site/ as static files
set -eu

cd "$(dirname "$0")/.."
cargo build --release --quiet
binary=target/release/reviewers
version=$("$binary" --version | awk '{ print $2 }')
size=$(wc -c <"$binary" | awk '{ printf "%.1f MB", $1 / 1000000 }')

cp install.sh site/install
{
  "$binary" help --agent
  for topic in writing evals onboarding hooks classifier; do
    printf '\n---\n\n'
    "$binary" help "$topic"
  done
} >site/docs.md
cp site/docs.md site/llms.txt

# The homepage states the version and size of the binary it installs.
sed -i.bak \
  -e "s|<span id=\"version\">[^<]*</span>|<span id=\"version\">v$version</span>|" \
  -e "s|<span class=\"size-text\">[^<]*</span>|<span class=\"size-text\">$size</span>|g" \
  site/index.html
rm -f site/index.html.bak
echo "site/ is ready: v$version, $size"
