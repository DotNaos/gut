#!/usr/bin/env sh
set -eu

repo="https://github.com/DotNaos/gut.git"
install_dir="${GUT_INSTALL_DIR:-$HOME/.local/bin}"
metadata_file="$install_dir/.gut-version"

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
new_timestamp="$(git -C "$tmp/gut" show -s --format='%ci' HEAD | awk '{ print $1, substr($2, 1, 5), $3 }')"
old_commit="${GUT_UPGRADE_FROM:-}"
old_version="${GUT_UPGRADE_FROM_VERSION:-}"
old_timestamp="${GUT_UPGRADE_FROM_TIMESTAMP:-}"

if [ -f "$metadata_file" ]; then
  metadata_version="$(awk -F= '$1 == "version" { print substr($0, index($0, "=") + 1); exit }' "$metadata_file")"
  metadata_commit="$(awk -F= '$1 == "commit" { print substr($0, index($0, "=") + 1); exit }' "$metadata_file")"
  metadata_timestamp="$(awk -F= '$1 == "timestamp" { print substr($0, index($0, "=") + 1); exit }' "$metadata_file")"

  [ -n "$metadata_version" ] && old_version="$metadata_version"
  [ -n "$metadata_commit" ] && old_commit="$metadata_commit"
  [ -n "$metadata_timestamp" ] && old_timestamp="$metadata_timestamp"
fi

if [ -z "$old_version" ] && [ -x "$install_dir/gut" ]; then
  old_version="$("$install_dir/gut" --version 2>/dev/null | awk '{print $2}' || true)"
fi

if [ -z "$old_timestamp" ] && [ -n "$old_commit" ] && [ "$old_commit" != "unknown" ]; then
  git -C "$tmp/gut" fetch --quiet --unshallow origin 2>/dev/null || true
  old_timestamp="$(git -C "$tmp/gut" show -s --format='%ci' "$old_commit" 2>/dev/null | awk '{ print $1, substr($2, 1, 5), $3 }' || true)"
fi

printf '\n============================================================\n'
if [ -n "$old_commit" ]; then
  printf '                       GUT UPGRADE\n'
  printf '============================================================\n'
  printf '  FROM  v%-10s  %-7s  %s\n' "${old_version:-unknown}" "$old_commit" "${old_timestamp:-timestamp unknown}"
  printf '  TO    v%-10s  %-7s  %s\n' "$new_version" "$new_commit" "$new_timestamp"
else
  printf '                       GUT INSTALL\n'
  printf '============================================================\n'
  printf '  VERSION  v%s\n' "$new_version"
  printf '  COMMIT   %s\n' "$new_commit"
  printf '  DATE     %s\n' "$new_timestamp"
fi
printf '============================================================\n\n'

GUT_BUILD_COMMIT="$new_commit" cargo build --release --manifest-path "$tmp/gut/Cargo.toml"

mkdir -p "$install_dir"
new_binary="$install_dir/.gut.new.$$"
cp "$tmp/gut/target/release/gut" "$new_binary"
chmod +x "$new_binary"
mv -f "$new_binary" "$install_dir/gut"

new_metadata="$install_dir/.gut-version.new.$$"
printf 'version=%s\ncommit=%s\ntimestamp=%s\n' "$new_version" "$new_commit" "$new_timestamp" > "$new_metadata"
mv -f "$new_metadata" "$metadata_file"

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
printf '  COMMIT     %s\n' "$new_timestamp"
printf '  PATH       %s/gut\n' "$install_dir"
printf '============================================================\n'
