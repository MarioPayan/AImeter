#!/usr/bin/env sh
# Install aimeter and wire it into your Claude Code statusline.
#
# Nothing here overwrites anything. An existing statusline script is backed up
# before a line is appended to it, an existing statusLine setting is left exactly
# as it is, and running this twice does nothing the second time.
#
#   AIMETER_DEST=/somewhere   install the binary elsewhere (default ~/.local/bin)
#   --no-wire                 install the binary only, print the snippet instead

set -eu

REPO="MarioPayan/aimeter"
DEST="${AIMETER_DEST:-$HOME/.local/bin}"
CLAUDE_DIR="$HOME/.claude"
SL="$CLAUDE_DIR/statusline.sh"
SETTINGS="$CLAUDE_DIR/settings.json"
WIRE=1

for arg in "$@"; do
  case "$arg" in
    --no-wire) WIRE=0 ;;
    -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "aimeter: unknown option $arg" >&2; exit 2 ;;
  esac
done

# ── the binary ───────────────────────────────────────────────────────────────
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

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "aimeter: fetching $target"
if ! curl -fsSL "https://github.com/$REPO/releases/latest/download/aimeter-$target.tar.gz" \
     -o "$tmp/aimeter.tar.gz"; then
  echo "aimeter: download failed." >&2
  echo "Build it instead:  cargo install --git https://github.com/$REPO" >&2
  exit 1
fi

tar -xzf "$tmp/aimeter.tar.gz" -C "$tmp"
mkdir -p "$DEST"
mv "$tmp/aimeter" "$DEST/aimeter"
chmod +x "$DEST/aimeter"
echo "aimeter: installed $("$DEST/aimeter" --version) to $DEST/aimeter"

snippet="printf '  %s' \"\$($DEST/aimeter line)\""

if [ "$WIRE" -eq 0 ]; then
  cat <<EOF

Add this to the script your statusLine runs:

  $snippet

EOF
  exit 0
fi

# ── the statusline script ────────────────────────────────────────────────────
mkdir -p "$CLAUDE_DIR"

if [ ! -f "$SL" ]; then
  cat > "$SL" <<EOF
#!/usr/bin/env bash
# Claude Code statusline.
#
# exec, so aimeter inherits this script's stdin. That payload is where the model,
# the reasoning effort and the context window come from — without it the segment
# still prints, but only the limits half of it.
exec "$DEST/aimeter" line
EOF
  chmod +x "$SL"
  echo "aimeter: wrote $SL"
elif grep -q 'aimeter' "$SL" 2>/dev/null; then
  echo "aimeter: $SL already runs aimeter — left alone"
else
  cp "$SL" "$SL.before-aimeter"
  {
    echo ""
    echo "# aimeter — appended by its installer"
    echo "$snippet"
  } >> "$SL"
  echo "aimeter: appended to $SL (backup: $SL.before-aimeter)"
  echo "aimeter: if the model and context part of the segment is missing, something"
  echo "         earlier in that script consumed stdin — capture it at the top and"
  echo "         pass it through to aimeter."
fi

# ── settings.json ────────────────────────────────────────────────────────────
# Only ever adds statusLine when there is none. Someone who already has one has
# made a choice, and an installer that overrules it is a bug.
if [ -f "$SETTINGS" ] && grep -q '"statusLine"' "$SETTINGS" 2>/dev/null; then
  echo "aimeter: settings.json already sets statusLine — left alone"
elif command -v python3 >/dev/null 2>&1; then
  python3 - "$SETTINGS" "$SL" <<'PY'
import json, os, shutil, sys
path, script = sys.argv[1], sys.argv[2]
data = {}
if os.path.exists(path):
    try:
        with open(path) as f:
            data = json.load(f)
    except (ValueError, OSError):
        print("aimeter: settings.json is not readable JSON — add statusLine yourself:")
        print(f'  "statusLine": {{ "type": "command", "command": "bash {script}" }}')
        raise SystemExit(0)
    shutil.copyfile(path, path + ".before-aimeter")
if not isinstance(data, dict):
    raise SystemExit(0)
data["statusLine"] = {"type": "command", "command": f'bash "{script}"', "refreshInterval": 10}
os.makedirs(os.path.dirname(path), exist_ok=True)
tmp = path + ".tmp"
with open(tmp, "w") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
os.replace(tmp, path)
print(f"aimeter: set statusLine in {path}")
PY
else
  cat <<EOF
aimeter: no python3 here, so add this to $SETTINGS yourself:

  "statusLine": { "type": "command", "command": "bash \"$SL\"" }
EOF
fi

case ":$PATH:" in
  *":$DEST:"*) ;;
  *) echo "aimeter: note — $DEST is not on your PATH (the statusline uses the full path, so it works anyway)" ;;
esac

echo "aimeter: done. Open a new Claude Code session to see it."
