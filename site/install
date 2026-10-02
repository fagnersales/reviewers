#!/bin/sh
# curl -fsSL https://reviewers.sh/install | sh
#
# Installs the reviewers binary into ~/.reviewers/bin, puts it on PATH, gives
# every coding agent on the machine the reviewers skill, and starts the first
# run. Set REVIEWERS_NO_ONBOARD=1 to stop after installing.
set -eu

MANIFEST="${REVIEWERS_RELEASES_URL:-https://reviewers.sh/releases/latest.txt}"
HOME_DIR="${REVIEWERS_HOME:-$HOME/.reviewers}"
BIN_DIR="$HOME_DIR/bin"

say() { printf '%s\n' "$*"; }
fail() { printf 'reviewers: %s\n' "$*" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || fail "curl is needed to install"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
  Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-gnu ;;
  *) fail "there's no build for $(uname -s) $(uname -m) yet" ;;
esac

manifest=$(curl -fsSL "$MANIFEST") || fail "cannot reach $MANIFEST"
version=$(printf '%s\n' "$manifest" | awk '$1 == "version" { print $2 }')
line=$(printf '%s\n' "$manifest" | awk -v t="$target" '$1 == t { print $2, $3 }')
[ -n "$line" ] || fail "release ${version} has no build for ${target}"
url=${line% *}
sha=${line#* }

tmp=$(mktemp "${TMPDIR:-/tmp}/reviewers.XXXXXX")
trap 'rm -f "$tmp"' EXIT
say "Downloading reviewers ${version} for ${target}…"
curl -fsSL "$url" -o "$tmp" || fail "download failed: $url"

if command -v shasum >/dev/null 2>&1; then
  actual=$(shasum -a 256 "$tmp" | awk '{ print $1 }')
else
  actual=$(sha256sum "$tmp" | awk '{ print $1 }')
fi
[ "$actual" = "$sha" ] || fail "the download doesn't match its checksum; nothing was installed"

mkdir -p "$BIN_DIR"
chmod +x "$tmp"
mv "$tmp" "$BIN_DIR/reviewers"
trap - EXIT

# PATH: one line in the shell's own startup file, written once.
line_marker="# added by reviewers"
case "${SHELL:-}" in
  */zsh) rc="$HOME/.zshrc" ;;
  */bash) if [ "$(uname -s)" = Darwin ]; then rc="$HOME/.bash_profile"; else rc="$HOME/.bashrc"; fi ;;
  */fish) rc="$HOME/.config/fish/conf.d/reviewers.fish" ;;
  *) rc="$HOME/.profile" ;;
esac
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    if ! grep -qs "$line_marker" "$rc"; then
      mkdir -p "$(dirname "$rc")"
      case "$rc" in
        *.fish) printf '\nfish_add_path %s %s\n' "$BIN_DIR" "$line_marker" >>"$rc" ;;
        *) printf '\nexport PATH="%s:$PATH" %s\n' "$BIN_DIR" "$line_marker" >>"$rc" ;;
      esac
    fi
    say "Added ${BIN_DIR} to PATH in ${rc} (open a new terminal to use it)."
    ;;
esac

"$BIN_DIR/reviewers" skill install
say ""
say "reviewers ${version} is installed."

# `curl | sh` gives the script the download as stdin; the first run needs the terminal instead.
if [ "${REVIEWERS_NO_ONBOARD:-}" != 1 ] && [ -t 1 ] && [ -r /dev/tty ]; then
  say ""
  exec "$BIN_DIR/reviewers" onboard </dev/tty
fi
say "Run \`reviewers\` in a terminal to set it up from your agent sessions."
