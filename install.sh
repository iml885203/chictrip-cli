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
if [ "$version" = "latest" ]; then
  url="https://github.com/${repository}/releases/latest/download/${archive}"
else
  url="https://github.com/${repository}/releases/download/${version}/${archive}"
fi

temp_dir="$(mktemp -d)"
trap 'rm -rf "$temp_dir"' EXIT HUP INT TERM

echo "Downloading chictrip ${version} for ${target}..."
curl --proto '=https' --tlsv1.2 --fail --location "$url" --output "$temp_dir/$archive"
tar -xzf "$temp_dir/$archive" -C "$temp_dir"
mkdir -p "$install_dir"
install -m 755 "$temp_dir/chictrip" "$install_dir/chictrip"

echo "Installed chictrip to $install_dir/chictrip"
case ":$PATH:" in
  *":$install_dir:"*) ;;
  *) echo "Add $install_dir to PATH before running chictrip." ;;
esac

