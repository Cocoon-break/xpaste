#!/usr/bin/env bash
set -euo pipefail
pkg=$(node -p "require('./package.json').version")
cargo=$(cargo metadata --no-deps --format-version 1 | node -e '
  let d = ""; process.stdin.on("data", c => d += c).on("end", () => {
    console.log(JSON.parse(d).packages.find(p => p.name === "xpaste").version) })')
if [ "$pkg" != "$cargo" ]; then
  echo "版本号不一致：package.json=${pkg}，Cargo=${cargo}" >&2; exit 1
fi
echo "版本号一致：${pkg}"
