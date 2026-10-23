#!/usr/bin/env bash
set -euo pipefail

out_dir="${1:-$HOME/burrow-macos-signing}"
name="burrow"

if [ -e "$out_dir" ]; then
  echo "refusing to overwrite $out_dir: a new certificate makes every Mac ask again for the firewall" >&2
  exit 1
fi

mkdir -p "$out_dir"
chmod 700 "$out_dir"

key="$out_dir/signing.key"
cert="$out_dir/signing.cer"
p12="$out_dir/signing.p12"
password_file="$out_dir/macos-certificate-password.txt"
base64_file="$out_dir/macos-certificate.base64.txt"

openssl rand -base64 24 | tr -d '\n' >"$password_file"
password="$(cat "$password_file")"

openssl req -x509 -newkey rsa:4096 -sha256 -days 7300 -nodes \
  -keyout "$key" -out "$cert" \
  -subj "/CN=$name/O=$name" \
  -addext "basicConstraints=critical,CA:true" \
  -addext "keyUsage=critical,digitalSignature,keyCertSign" \
  -addext "extendedKeyUsage=critical,codeSigning" \
  2>/dev/null

openssl pkcs12 -export -legacy \
  -inkey "$key" -in "$cert" -out "$p12" \
  -name "$name" -passout "pass:$password"

base64 -w0 "$p12" >"$base64_file" 2>/dev/null || base64 -i "$p12" | tr -d '\n' >"$base64_file"

chmod 600 "$key" "$p12" "$password_file" "$base64_file"

openssl x509 -in "$cert" -noout -subject -enddate -fingerprint -sha256
echo
echo "set these repository secrets:"
echo "  MACOS_CERTIFICATE           <- $base64_file"
echo "  MACOS_CERTIFICATE_PASSWORD  <- $password_file"
echo
echo "keep a backup of $out_dir outside git, and never replace it"
