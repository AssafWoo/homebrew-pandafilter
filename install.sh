#!/usr/bin/env bash
# PandaFilter installer
# On macOS: installs via Homebrew (prebuilt binary, takes seconds).
# On Linux / no-brew: builds from source via cargo (~1 min on first run).
set -e

REPO_URL="https://github.com/AssafWoo/PandaFilter.git"
CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"

# ── macOS: prefer Homebrew ────────────────────────────────────────────────────

if [[ "$(uname)" == "Darwin" ]] && command -v brew &>/dev/null; then
  brew tap assafwoo/pandafilter 2>/dev/null || true

  # Detect the "64" bad-keg: older installs stored the keg as version "64"
  # (inferred from "arm64" in the asset URL). brew upgrade skips it because
  # 64 > 0.5.x. Force a reinstall to fix the keg name once and for all.
  CELLAR="$(brew --cellar assafwoo/pandafilter/pandafilter 2>/dev/null || true)"
  if [[ -n "$CELLAR" && -d "$CELLAR/64" ]]; then
    echo "Detected stale keg (version \"64\") — reinstalling to fix..."
    brew reinstall assafwoo/pandafilter/pandafilter
  elif brew list assafwoo/pandafilter/pandafilter &>/dev/null 2>&1; then
    brew upgrade assafwoo/pandafilter/pandafilter || true
  else
    brew install assafwoo/pandafilter/pandafilter
  fi

  echo ""
  echo "PandaFilter installed. You're all set — hooks are registered automatically."
  exit 0
fi

# ── Linux / no-brew: build from source ───────────────────────────────────────

if ! command -v cargo &>/dev/null; then
  echo "Rust not found — installing rustup..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
  # shellcheck source=/dev/null
  source "$HOME/.cargo/env"
fi

echo "Building PandaFilter from source (this takes ~1 min on first run)..."
cargo install --git "$REPO_URL" --bin panda --locked 2>&1

# ── Ensure ~/.cargo/bin is on PATH ────────────────────────────────────────────

add_to_path() {
  local rc="$1"
  local line='export PATH="$HOME/.cargo/bin:$PATH"'
  if [ -f "$rc" ] && ! grep -qF '.cargo/bin' "$rc"; then
    echo "" >> "$rc"
    echo "# Added by PandaFilter installer" >> "$rc"
    echo "$line" >> "$rc"
    echo "  → Added cargo/bin to $rc"
  fi
}

if ! echo "$PATH" | grep -q '.cargo/bin'; then
  echo ""
  echo "Adding ~/.cargo/bin to PATH in your shell config..."
  add_to_path "$HOME/.bashrc"
  add_to_path "$HOME/.zshrc"
  add_to_path "$HOME/.profile"
  export PATH="$CARGO_BIN:$PATH"
  echo "  (effective now in this session)"
fi

# ── Register Claude Code hooks ────────────────────────────────────────────────

echo ""
if command -v panda &>/dev/null; then
  panda init && echo "Claude Code hooks registered."
elif [ -x "$CARGO_BIN/panda" ]; then
  "$CARGO_BIN/panda" init && echo "Claude Code hooks registered."
else
  echo "Run 'panda init' to register Claude Code hooks."
fi

echo ""
echo "PandaFilter installed. Open a new terminal (or run: source ~/.cargo/env) and you're set."
