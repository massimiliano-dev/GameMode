#!/bin/sh
# Type-checks the Windows build from macOS/Linux.
#
# tauri-build needs llvm-rc to compile Windows resources; `cargo check` never
# links, so a stand-in that writes an empty output file is enough. Not usable
# for real builds — those run in .github/workflows/build-windows.yml.
set -e
rustup target add x86_64-pc-windows-msvc >/dev/null 2>&1 || true
stub=$(mktemp -d)
trap 'rm -rf "$stub"' EXIT
cat > "$stub/llvm-rc" <<'STUB'
#!/bin/sh
for a in "$@"; do
  if [ "$a" = "/?" ]; then echo "OVERVIEW: Resource Converter"; echo "  /no-preprocess"; exit 0; fi
done
out=""; prev=""
for a in "$@"; do
  if [ "$prev" = "/fo" ]; then out="$a"; fi
  case "$a" in /fo?*) out="${a#/fo}";; esac
  prev="$a"
done
[ -n "$out" ] && : > "$out"
exit 0
STUB
chmod +x "$stub/llvm-rc"
cd "$(dirname "$0")/../src-tauri"
PATH="$stub:$PATH" cargo check --target x86_64-pc-windows-msvc "$@"
