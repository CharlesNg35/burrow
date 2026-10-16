#!/usr/bin/env bash
set -euo pipefail

BENCH_DIR="${BENCH_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/burrow-bench}"
mkdir -p "$BENCH_DIR/dl" "$BENCH_DIR/bin"
cd "$BENCH_DIR/dl"

fetch() {
  local url="$1" sum="$2" file
  file="$(basename "$url")"
  [ -f "$file" ] || curl -fsSL "$url" -o "$file"
  echo "$sum  $file" | sha256sum -c --quiet -
}

fetch https://github.com/rathole-org/rathole/releases/download/v0.5.0/rathole-x86_64-unknown-linux-gnu.zip \
  3e7d0d0f365120cd3cd351d147d1a12ee960c8068b464d4dd533a3821873b80e
fetch https://github.com/fatedier/frp/releases/download/v0.71.1/frp_0.71.1_linux_amd64.tar.gz \
  1d9875210972a5d1a62b2dc4c48df45ab0af05d3ca1b5e494a014428259df803
fetch https://github.com/jpillora/chisel/releases/download/v1.12.0/chisel_1.12.0_linux_amd64.gz \
  f3f180f1d93aa72cce4e6386f98cc06569a0146fbd65eb4423cf83e6434bcfe6

python3 -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extract("rathole", sys.argv[2])' rathole-x86_64-unknown-linux-gnu.zip "$BENCH_DIR/bin"
tar -xzf frp_0.71.1_linux_amd64.tar.gz -C "$BENCH_DIR/bin" --strip-components=1 frp_0.71.1_linux_amd64/frps frp_0.71.1_linux_amd64/frpc
gzip -dc chisel_1.12.0_linux_amd64.gz > "$BENCH_DIR/bin/chisel"
chmod +x "$BENCH_DIR/bin/"*
echo "tools in $BENCH_DIR/bin: $(ls "$BENCH_DIR/bin" | tr '\n' ' ')"
