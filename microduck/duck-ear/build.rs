//! Where the sherpa-onnx libraries are found at run time.
//!
//! The binary links `libsherpa-onnx-c-api.so`, which itself links `libonnxruntime.so` —
//! two hops. Both files travel next to `duck-ear`, so both hops must be resolved by an
//! `$ORIGIN` rpath on the executable.
//!
//! `--disable-new-dtags` is load-bearing, not cosmetic. With the modern `DT_RUNPATH` tag,
//! a binary's rpath is consulted only for *its own* direct dependencies; when the loader
//! then resolves `libsherpa-onnx-c-api.so`'s need for `libonnxruntime.so`, the executable's
//! RUNPATH is invisible to it. The old `DT_RPATH` is inherited down the whole chain, which
//! is exactly what a two-hop layout needs. Getting this wrong does not fail at build time:
//! the loader finds `libonnxruntime.so` in the default paths, where the board's 1.28 lives,
//! and `duck-ear` dies at startup on a version error that names neither library.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    println!("cargo:rustc-link-arg-bins=-Wl,--disable-new-dtags");
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
}
