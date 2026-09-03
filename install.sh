#!/usr/bin/env sh
set -eu

repo="https://github.com/DotNaos/gut.git"
install_dir="${GUT_INSTALL_DIR:-$HOME/.local/bin}"

if ! command -v git >/dev/null 2>&1; then
  echo "gut: git is required" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "gut: cargo is required to build from source" >&2
  exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

git clone --depth 1 "$repo" "$tmp/gut"
cargo build --release --manifest-path "$tmp/gut/Cargo.toml"

mkdir -p "$install_dir"
cp "$tmp/gut/target/release/gut" "$install_dir/gut"
chmod +x "$install_dir/gut"

shell="$(basename "${SHELL:-}")"
case "$shell" in
  bash)
    completion_dir="$HOME/.local/share/bash-completion/completions"
    mkdir -p "$completion_dir"
    "$install_dir/gut" completions bash > "$completion_dir/gut"
    ;;
  zsh)
    completion_dir="$HOME/.zfunc"
    mkdir -p "$completion_dir"
    "$install_dir/gut" completions zsh > "$completion_dir/_gut"
    ;;
  fish)
    completion_dir="$HOME/.config/fish/completions"
    mkdir -p "$completion_dir"
    "$install_dir/gut" completions fish > "$completion_dir/gut.fish"
    ;;
esac

echo "Installed gut to $install_dir/gut"
