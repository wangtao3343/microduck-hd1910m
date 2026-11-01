#!/usr/bin/env bash
# Cross-compile `mediad` (camera + WebRTC) for the board.
#
# `scripts/build.sh` deliberately leaves `mediad` out: it is the only crate in the workspace with a
# *pkg-config* C dependency (gstreamer-rs), so plain `cargo build --target aarch64…` fails at
# `gstreamer-sys` unless pkg-config is pointed at the robot's own headers and libraries. That is
# what `microduck/scripts/cross-sysroot.sh` builds, and what this script expects:
#
#   sh microduck/scripts/cross-sysroot.sh          # first time, and after a userland change
#   bash scripts/build-mediad.sh                  # then this
#   bash scripts/build-mediad.sh --deploy         # …and rsync the binary to the board
#
# `cross-sysroot.sh` defaults to $TMPDIR/duck-aarch64-sysroot, which does not survive a reboot —
# point DUCK_SYSROOT at somewhere persistent if you rebuild often. The mirror variable exists
# because deb.debian.org is ~34 KB/s from here and the Tsinghua mirror is ~9 MB/s; both serve the
# same trixie packages.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "$0")/.." && pwd)"
SYSROOT="${DUCK_SYSROOT:-${TMPDIR:-/tmp}/duck-aarch64-sysroot}"

# rustup lives in ~/.cargo/bin here and is not on a non-login shell's PATH; `scripts/build.sh`
# inherits an interactive shell that has it, this one may not.
export PATH="$HOME/.cargo/bin:$PATH"
DEPLOY=0

for arg in "$@"; do
  case "$arg" in
    --deploy) DEPLOY=1 ;;
    *) printf 'Usage: bash scripts/build-mediad.sh [--deploy]\n' >&2; exit 2 ;;
  esac
done

[ -d "$SYSROOT/usr/lib/aarch64-linux-gnu/pkgconfig" ] || {
  printf 'no aarch64 sysroot at %s\n' "$SYSROOT" >&2
  printf 'Build it first:\n  sh microduck/scripts/cross-sysroot.sh\n' >&2
  exit 1
}

# Same four variables `cross-sysroot.sh` prints, plus the linker. `PKG_CONFIG_LIBDIR` *replaces*
# the search path — additive `PKG_CONFIG_PATH` would answer with the host's GStreamer and produce a
# binary that links here and dies on the board.
export PKG_CONFIG_SYSROOT_DIR="$SYSROOT"
export PKG_CONFIG_LIBDIR="$SYSROOT/usr/lib/aarch64-linux-gnu/pkgconfig:$SYSROOT/usr/share/pkgconfig"
export PKG_CONFIG_ALLOW_CROSS=1
# `--allow-shlib-undefined`: GNU ld verifies that every undefined symbol in the shared libraries
# it links is resolvable *on the link line*, and GLib's DT_NEEDED chain (pcre2, ffi, mount,
# selinux, zlib, orc) is not on it — pkg-config never puts a `Requires.private` there. Those
# libraries are present on the board, so the symbols resolve at run time and the check is the
# false positive it looks like. `cargo board` does not hit this because `cargo zigbuild` links
# with lld, which does not perform the check. Adding each library by hand would work too and
# would have to grow every time GLib does.
export RUSTFLAGS="${RUSTFLAGS:-} -L $SYSROOT/usr/lib/aarch64-linux-gnu -C link-arg=-Wl,--allow-shlib-undefined"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
export CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++

(cd "$ROOT/microduck" && cargo build --release --locked --target aarch64-unknown-linux-gnu -p mediad)

install -d "$ROOT/out/arm64/bin"
install -m755 "$ROOT/microduck/target/aarch64-unknown-linux-gnu/release/mediad" \
  "$ROOT/out/arm64/bin/mediad"
printf 'Built %s\n' "$ROOT/out/arm64/bin/mediad"

# The unit and the board-local config live outside `out/`, so deployment is two files and a
# binary. `--deploy` only copies; installing the unit and restarting stays the operator's call.
if [ "$DEPLOY" = 1 ]; then
  HOST="${DUCK_HOST:-duck@192.168.3.10}"
  DIR="${DUCK_DIR:-/home/duck/workspace/microduck-hd1910m}"
  scp "$ROOT/out/arm64/bin/mediad" "$HOST:$DIR/out/arm64/bin/mediad"
  scp "$ROOT/microduck/mediad/systemd/mediad-runtime.service" "$HOST:$DIR/local/mediad.service"
  printf 'Copied mediad and mediad.service to %s:%s\n' "$HOST" "$DIR"
  printf 'On the board (root):\n'
  printf '  sudo install -m644 %s/local/mediad.service /etc/systemd/system/mediad.service\n' "$DIR"
  printf '  sudo systemctl daemon-reload && sudo systemctl enable --now mediad\n' "$DIR"
fi
