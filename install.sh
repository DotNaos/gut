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
new_commit="$(git -C "$tmp/gut" rev-parse --short HEAD)"
new_version="$(awk -F '"' '/^version = "/ { print $2; exit }' "$tmp/gut/Cargo.toml")"
old_commit="${GUT_UPGRADE_FROM:-}"
old_version="${GUT_UPGRADE_FROM_VERSION:-}"

if [ -z "$old_version" ] && [ -x "$install_dir/gut" ]; then
  old_version="$("$install_dir/gut" --version 2>/dev/null | awk '{print $2}' || true)"
fi

printf '\n============================================================\n'
if [ -n "$old_commit" ]; then
  printf '                       GUT UPGRADE\n'
  printf '============================================================\n'
  printf '  FROM  v%-12s  %s\n' "${old_version:-unknown}" "$old_commit"
  printf '  TO    v%-12s  %s\n' "$new_version" "$new_commit"
else
  printf '                       GUT INSTALL\n'
  printf '============================================================\n'
  printf '  VERSION  v%s\n' "$new_version"
  printf '  COMMIT   %s\n' "$new_commit"
fi
printf '============================================================\n\n'

GUT_BUILD_COMMIT="$new_commit" cargo build --release --manifest-path "$tmp/gut/Cargo.toml"

mkdir -p "$install_dir"
new_binary="$install_dir/.gut.new.$$"
cp "$tmp/gut/target/release/gut" "$new_binary"
chmod +x "$new_binary"
mv -f "$new_binary" "$install_dir/gut"

shell="$(basename "${SHELL:-}")"
case "$shell" in
  bash)
    completion_dir="$HOME/.local/share/bash-completion/completions"
    mkdir -p "$completion_dir"
    "$install_dir/gut" completions bash > "$completion_dir/gut"
    echo "Installed bash completions to $completion_dir/gut"
    ;;
  zsh)
    completion_dir="$HOME/.zsh/completions"
    mkdir -p "$completion_dir"
    "$install_dir/gut" completions zsh > "$completion_dir/_gut"
    echo "Installed zsh completions to $completion_dir/_gut"
    ;;
  fish)
    completion_dir="$HOME/.config/fish/completions"
    mkdir -p "$completion_dir"
    "$install_dir/gut" completions fish > "$completion_dir/gut.fish"
    echo "Installed fish completions to $completion_dir/gut.fish"
    ;;
esac

printf '\n============================================================\n'
printf '  INSTALLED  gut v%s (%s)\n' "$new_version" "$new_commit"
printf '  PATH       %s/gut\n' "$install_dir"
printf '============================================================\n'
