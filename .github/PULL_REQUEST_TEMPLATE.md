## Summary

<!-- Briefly describe what this PR changes and why. -->

## Type of Change

- [ ] Bug fix
- [ ] New feature / enhancement
- [ ] Performance / rendering / memory improvement
- [ ] Documentation / CI / packaging

## Verification Checklist

- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes cleanly
- [ ] `cargo test --all-features --no-fail-fast` passes cleanly
- [ ] Any new or modified `unsafe` block/fn has a clear `// SAFETY:` comment explaining the invariant
- [ ] Unit tests added or updated for non-trivial logic changes
