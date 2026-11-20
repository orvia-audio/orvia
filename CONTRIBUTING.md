# Contributing

Write code comments, Rustdoc and commits in English. Keep each commit focused. Document exported Rust items, including units and failure conditions. Give keyboard and pointer input the same behavior and keep focus visible.

## Checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo doc --workspace --no-deps --locked
```

The Linux build resolves `xattr 0.2.3` through GPUI's `zed-async-tar` dependency. It uses the Linux `libc::ENOATTR` alias removed in libc 0.2.190. Keep libc at 0.2.189 and mio at 1.2.4 while this dependency remains. Mio 1.2.5 requires libc 0.2.190 or newer.

## UI snapshots

Ubuntu 24.04 CI renders each mixer screen in English and Japanese at 96 DPI. The fixtures use synthetic devices, a fixed waveform and a silent recording. Install the Linux dependencies from the desktop workflow and run:

```sh
sh tests/ui/run.sh
```

A mismatch saves the actual image, the previous image and a highlighted diff under `target/ui-snapshots/`. CI uploads them as `ui-snapshot-differences`. For an intentional change, append `--update` and review the changed PNGs in [the baseline directory](tests/ui/snapshots). Add a fixture and a capture case when adding a screen.

## Workflow dependencies

After editing an action reference, run `gh actions-lock` and commit the generated lockfile with the workflow. Run `gh actions-lock --verify` to check the pins.

## Versions

Use `X.Y.Z-snapshot.N` for distributed development builds and tag stable releases as `vX.Y.Z`. Update the workspace version and lockfile together. Increment `CFBundleVersion` in the macOS Info.plist for each distributed build. Within a Windows Installer series, snapshots use their sequence number as the third numeric field and stable uses 65535. Advance the series for the next release.

The project skills for [Rust](.agents/skills/rust-best-practices/SKILL.md) and [GPUI actions](.agents/skills/gpui-action/SKILL.md) are installed with GitHub CLI. [Packaging and platform checks](docs/testing.md)
