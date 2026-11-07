#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "$0")/.." && pwd)"
MODE="${1:-native}"
case "$MODE" in
  native) ARGS=(build --release --locked); SUB=release; TARGET=x86_64-unknown-linux-gnu ;;
  arm64)
    export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
    export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
    export CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++
    ARGS=(build --release --locked --target aarch64-unknown-linux-gnu)
    SUB=aarch64-unknown-linux-gnu/release; TARGET=aarch64-unknown-linux-gnu ;;
  *) printf 'Usage: bash scripts/build.sh [native|arm64]\n' >&2; exit 2 ;;
esac
(cd "$ROOT/microduck" && cargo "${ARGS[@]}" -p robotd -p robotctl -p voice -p duck-ear)
(cd "$ROOT/microduck_app/backend" && cargo "${ARGS[@]}")
mkdir -p "$ROOT/out/$MODE/bin"
install -m755 "$ROOT/microduck/target/$SUB/robotd" "$ROOT/microduck/target/$SUB/robotctl" \
  "$ROOT/microduck/target/$SUB/duck-voice" "$ROOT/microduck/target/$SUB/duck-ear" "$ROOT/out/$MODE/bin/"
install -m755 "$ROOT/microduck_app/backend/target/$SUB/microduck-app-server" "$ROOT/out/$MODE/bin/"

# duck-ear links sherpa-onnx's C API, which carries its own ONNX Runtime. Both .so files are
# found through the binary's `$ORIGIN` rpath (see microduck/duck-ear/build.rs), so they are
# staged next to the binaries rather than in a system lib dir: the board's own
# libonnxruntime belongs to robotd and must not be shadowed. They come from sherpa-rs-sys's
# download cache, keyed by checksum — hence the glob rather than a fixed path.
SHERPA_LIB_DIR=$(ls -d "$HOME/.cache/sherpa-rs/$TARGET"/*/sherpa-onnx-*/lib 2>/dev/null | head -1)
if [ -z "$SHERPA_LIB_DIR" ]; then
  printf 'error: sherpa-onnx is not in ~/.cache/sherpa-rs/%s — build duck-ear once with network access\n' \
    "$TARGET" >&2
  exit 1
fi
install -m755 "$SHERPA_LIB_DIR/libsherpa-onnx-c-api.so" "$SHERPA_LIB_DIR/libonnxruntime.so" \
  "$ROOT/out/$MODE/bin/"
printf 'Built %s\n' "$ROOT/out/$MODE/bin"
