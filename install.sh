#!/bin/sh
set -eu

repository="iml885203/chictrip-cli"
install_dir="${CHICTRIP_INSTALL_DIR:-$HOME/.local/bin}"
version="${CHICTRIP_VERSION:-latest}"

case "$(uname -s)" in
  Darwin) target="universal-apple-darwin" ;;
  Linux)
    case "$(uname -m)" in
      x86_64 | amd64) target="x86_64-unknown-linux-gnu" ;;
      arm64 | aarch64) target="aarch64-unknown-linux-gnu" ;;
      *) echo "Unsupported Linux architecture: $(uname -m)" >&2; exit 1 ;;
    esac
    ;;
  *) echo "This installer supports macOS and Linux. Use the Windows zip from GitHub Releases." >&2; exit 1 ;;
esac

archive="chictrip-${target}.tar.gz"
checksum="chictrip-${target}.sha256"
if [ "$version" = "latest" ]; then
  base_url="https://github.com/${repository}/releases/latest/download"
else
  base_url="https://github.com/${repository}/releases/download/${version}"
fi

temp_dir="$(mktemp -d)"
trap 'rm -rf "$temp_dir"' EXIT HUP INT TERM

echo "Downloading chictrip ${version} for ${target}..."
curl --proto '=https' --tlsv1.2 --fail --location "$base_url/$archive" --output "$temp_dir/$archive"
curl --proto '=https' --tlsv1.2 --fail --location "$base_url/$checksum" --output "$temp_dir/$checksum"
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$temp_dir" && sha256sum -c "$checksum")
elif command -v shasum >/dev/null 2>&1; then
  (cd "$temp_dir" && shasum -a 256 -c "$checksum")
else
  echo "A SHA-256 tool (sha256sum or shasum) is required." >&2
  exit 1
fi
tar -xzf "$temp_dir/$archive" -C "$temp_dir"
mkdir -p "$install_dir"
install -m 755 "$temp_dir/chictrip" "$install_dir/chictrip"

echo "Installed chictrip to $install_dir/chictrip"
case ":$PATH:" in
  *":$install_dir:"*) ;;
  *) echo "Add $install_dir to PATH before running chictrip." ;;
esac
