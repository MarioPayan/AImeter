#!/usr/bin/env sh
# Install aimeter: fetch the prebuilt binary for this platform, put it on PATH,
# and print the snippet you need. Nothing to build, no Rust toolchain required.
#
# It deliberately does NOT edit your statusline script. Claude Code composes a
# single statusLine command out of however many segments you run, and silently
# rewriting that file is not a thing an installer should do to you.

set -eu

REPO="MarioPayan/aimeter"
DEST="${AIMETER_DEST:-$HOME/.local/bin}"

os=$(uname -s)
arch=$(uname -m)
case "$os-$arch" in
  Linux-x86_64) target="x86_64-unknown-linux-gnu" ;;
  Darwin-arm64) target="aarch64-apple-darwin" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  *)
    echo "aimeter: no prebuilt binary for $os-$arch." >&2
    echo "Build it instead:  cargo install --git https://github.com/$REPO" >&2
    exit 1
    ;;
esac

url="https://github.com/$REPO/releases/latest/download/aimeter-$target.tar.gz"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "aimeter: fetching $target"
if ! curl -fsSL "$url" -o "$tmp/aimeter.tar.gz"; then
  echo "aimeter: could not download $url" >&2
  echo "Build it instead:  cargo install --git https://github.com/$REPO" >&2
  exit 1
fi

tar -xzf "$tmp/aimeter.tar.gz" -C "$tmp"
mkdir -p "$DEST"
mv "$tmp/aimeter" "$DEST/aimeter"
chmod +x "$DEST/aimeter"

echo "aimeter: installed to $DEST/aimeter"
"$DEST/aimeter" --version

case ":$PATH:" in
  *":$DEST:"*) ;;
  *) echo "aimeter: note — $DEST is not on your PATH" ;;
esac

cat <<EOF

Add the segment to your statusline. Claude Code runs one statusLine command, so
compose it with whatever else you already print:

  # ~/.claude/statusline.sh
  printf '%s' "\$($DEST/aimeter line)"

and point settings.json at that script:

  "statusLine": { "type": "command", "command": "bash ~/.claude/statusline.sh" }

EOF
