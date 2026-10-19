#!/usr/bin/env bash
# 下载界面字体（latin 子集 woff2）与 OFL 许可证到 src/fonts/。字体随安装包内置，运行时不联网。
set -euo pipefail
cd "$(dirname "$0")/.."

UA='Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36'
CSS_URL='https://fonts.googleapis.com/css2?family=Geist:wght@400..600&family=Unbounded:wght@500..700&family=JetBrains+Mono:wght@400..500&display=swap'
OUT=src/fonts
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -A "$UA" "$CSS_URL" -o "$tmp/fonts.css"

# 每个 @font-face 前有 /* 子集名 */ 注释；只取 latin 子集，输出“族名<TAB>woff2 地址”
awk '
  /^\/\* / { subset = $2 }
  /font-family:/ { match($0, /\x27[^\x27]+\x27/); family = substr($0, RSTART + 1, RLENGTH - 2) }
  /src: url\(/ && subset == "latin" { match($0, /url\([^)]+\)/); print family "\t" substr($0, RSTART + 4, RLENGTH - 5) }
' "$tmp/fonts.css" > "$tmp/urls.tsv"

fetch() { # 族名 文件名
  local url
  url=$(awk -F '\t' -v f="$1" '$1 == f { print $2; exit }' "$tmp/urls.tsv")
  [ -n "$url" ] || { echo "未找到 $1 的 latin 子集" >&2; exit 1; }
  curl -fsSL "$url" -o "$tmp/$2"
  [ -s "$tmp/$2" ] || { echo "$2 下载为空" >&2; exit 1; }
}
fetch 'Geist' geist.woff2
fetch 'Unbounded' unbounded.woff2
fetch 'JetBrains Mono' jetbrains-mono.woff2

curl -fsSL https://raw.githubusercontent.com/vercel/geist-font/main/LICENSE.txt -o "$tmp/LICENSE-geist.txt"
curl -fsSL https://raw.githubusercontent.com/googlefonts/unbounded/main/OFL.txt -o "$tmp/LICENSE-unbounded.txt"
curl -fsSL https://raw.githubusercontent.com/JetBrains/JetBrainsMono/master/OFL.txt -o "$tmp/LICENSE-jetbrains-mono.txt"

# 全部成功才落盘，不留半套文件
mkdir -p "$OUT"
mv "$tmp"/*.woff2 "$tmp"/LICENSE-*.txt "$OUT"/
ls -l "$OUT"
