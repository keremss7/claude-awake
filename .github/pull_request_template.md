## What and why

<!-- What changes, and what problem it solves. -->

## Checks

- [ ] `npm run lint`
- [ ] `cargo fmt --manifest-path src-tauri/Cargo.toml --check`
- [ ] `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml`
- [ ] The other platform still type-checks (see CONTRIBUTING.md)

## If this touches the privileged helper

<!-- Delete if it does not. -->

- [ ] No value derived from a request is executed
- [ ] No setting is written that is absent from the baseline snapshot
- [ ] Every new exit path restores the power settings
