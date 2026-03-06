## Summary

Describe the problem and the change in 2-5 bullets.

## Validation

- [ ] `cargo fmt --all`
- [ ] `cargo clippy --workspace -- -D warnings`
- [ ] `cargo test --workspace --exclude pb-integration-tests`
- [ ] `cargo test -p pb-integration-tests` (if behavior or storage changes require it)

## Risk

Call out any storage schema, replay ordering, deployment, or operational risk.

## Notes

Anything reviewers should read first.
