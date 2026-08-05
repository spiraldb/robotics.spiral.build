# Patched `block` 0.1.6

This is a source-compatible copy of the unmaintained `block` 0.1.6 crate used
by Nokhwa's current macOS dependency chain. Its opaque Objective-C block class
is represented as the 32-pointer storage exported by the Blocks runtime,
instead of an uninhabited enum. This removes Rust's `uninhabited_static`
future-incompatibility warning while preserving the runtime ABI.

The original crate is MIT-licensed and published at
<https://github.com/SSheldon/rust-block>. The upstream implementation is kept
source-compatible; the semantic patch is limited to the private `Class`
representation described above. See `LICENSE-MIT` for the license notice.
