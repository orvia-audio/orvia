# Contributing

Write code comments, Rustdoc and commits in English. Keep each commit focused. Document exported Rust items, including units and failure conditions. Give keyboard and pointer input the same behavior and keep focus visible.

## Checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo doc --workspace --no-deps --locked
```

## Versions

Use `0.1.0-snapshot.N` for distributed development builds. Reserve `v0.1.0` for the first stable release. Update the workspace version and lockfile together. Within a Windows Installer series, snapshots use their sequence number as the third numeric field and stable uses 65535. Advance the series for the next release.

The project skills for [Rust](.agents/skills/rust-best-practices/SKILL.md) and [GPUI actions](.agents/skills/gpui-action/SKILL.md) are installed with GitHub CLI. [Packaging and platform checks](docs/testing.md)
