#!/usr/bin/env bash
# build.sh - 构建 ar-ocg-router（Linux 单文件二进制，静态链接 libc 可选）
#
# 用法：
#   ./build.sh                 # 构建 Web 控制台 + release 二进制 + 打包到 dist/linux-x64
#   ./build.sh --musl          # 使用 x86_64-unknown-linux-musl（完全静态）
#
# 每次都会重新构建 Web 控制台再 embed。只改 Rust 时直接跑 cargo，不必走本脚本。
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$root"

target=""
for arg in "$@"; do
  case "$arg" in
    --musl) target="x86_64-unknown-linux-musl"; echo "==> using musl target (fully static)" ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

# The console is embedded into the binary, so its static export must exist first. It is rebuilt
# on every run: a staleness guard is more code than the build it protects, and getting it wrong
# is how an older console ships. For Rust-only iteration call cargo directly.
if [[ ! -d web/node_modules ]]; then
  echo "==> npm install (web/)"
  (cd web && npm install --no-audit --no-fund)
fi
echo "==> npm run build (web/ -> web/out)"
(cd web && npm run build)
if [[ ! -f web/out/index.html ]]; then
  echo "web/out/index.html is missing after the web build" >&2
  exit 1
fi

# Preflight: rustc needs a linker for the host target. On Linux that is cc/ld; on Windows
# under MSYS/Git Bash it must be an x64 link.exe, otherwise the build ends in hundreds of
# "unresolved external symbol" errors that look like a source problem.
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    linker="$(command -v link.exe || true)"
    if [[ -z "$linker" ]]; then
      echo "warning: no link.exe on PATH. Run from a Developer Command Prompt for VS," >&2
      echo "         or install Visual Studio Build Tools with the C++ workload." >&2
    fi
    ;;
esac

if [[ -n "$target" ]]; then
  cargo build --release --target "$target"
  bin="target/$target/release/ar-ocg-router"
else
  cargo build --release
  bin="target/release/ar-ocg-router"
fi

out="$root/dist/linux-x64"
mkdir -p "$out"
cp "$bin" "$out/ar-ocg-router"
cp config.example.yaml README.md README-zh.md "$out/"
cp -r docs "$out/"
cp assets/models.library.json "$out/"
# service registration files (systemd)
cp deploy/linux/install.sh deploy/linux/ar-ocg-router.service "$out/"
chmod +x "$out/install.sh"
chmod +x "$out/ar-ocg-router"
sha256sum "$out/ar-ocg-router" > "$out/ar-ocg-router.sha256" 2>/dev/null || true

echo ""
echo "OK  $out/ar-ocg-router  ($(du -h "$out/ar-ocg-router" | cut -f1))"
echo "部署：把 ar-ocg-router 与 config.yaml 放到同一目录，运行 ./ar-ocg-router --selftest 自检"
