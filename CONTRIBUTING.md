# Contributing to bbstats

Thank you for helping improve `bbstats`!

## Development Setup

- **Rust toolchain**: Stable Rust matching `rust-version` in `Cargo.toml` (`>= 1.88`), with `clippy` and `rustfmt` installed.
- **Windows**: Native Direct2D / DirectComposition Dynamic Island overlay compiles out of the box with `cargo build`.
- **macOS / Linux**: Builds the cross-platform live ANSI terminal HUD, `--status`, `--daemon`, and CLI tools (`run`, `port`, `launch`).

## Verification Before Submitting a PR

Run the same checks enforced by CI:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features --no-fail-fast
cargo build --release --locked
```

## Coding Guidelines

1. **Safety documentation**: Every `unsafe` block or `unsafe extern` FFI callback must include a concise `// SAFETY:` comment explaining the upheld invariant.
2. **No panics in runtime paths**: The release profile uses `panic = "abort"`. Avoid `.unwrap()`, unchecked slice indexing, and `f32::clamp` on potentially `NaN` inputs outside tests.
3. **Secret hygiene**: Never log or display unredacted secrets from process command lines; pass display cmdlines through `redact_secrets`.
4. **Tests**: Bug fixes and parser/resolver/UI layout changes should include targeted unit tests.
