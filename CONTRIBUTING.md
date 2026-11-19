# Contributing

Use English in source comments, Rustdoc, documentation and commit messages. Keep each commit focused on one reviewable behavior. Preserve the license and attribution of the imported Microsoft sample when changing the driver.

## Versioning

The first release is planned as `v0.1.0`. Set the workspace version to `0.1.0-snapshot.N` for development packages and increment only `N` for each new snapshot distributed. Keep the same number for ordinary commits between snapshots. At release time, remove the suffix and tag that commit `v0.1.0`. Regenerate `Cargo.lock` when the workspace version changes.

Windows Installer compares three numeric fields. All snapshots for the same target release share the `workspace.metadata.windows-installer.series` value. The third field is the snapshot number (1–65534), or 65535 for the stable release. When starting development for the next target release, advance the MSI series and restart the snapshot number at 1. The first series is `0.4` so users can upgrade from the earlier `0.3.1` development installer. [Windows Installer version comparison](https://learn.microsoft.com/en-us/windows/win32/msi/productversion)

## Rust and Rustdoc

Document each crate's role and each exported item's contract. State audio units, rates, frame lengths, ownership of state, failure conditions and any real-time constraints where the caller needs them. Use executable examples for normal usage and a Rustdoc Errors section for fallible public operations. Explain non-obvious implementation choices with ordinary comments. The workspace denies missing docs and broken intra-doc links. [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/documentation.html) · [Rustdoc Book](https://doc.rust-lang.org/rustdoc/how-to-write-documentation.html)

Run the same Rust checks as CI before submitting changes.

~~~powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
$env:RUSTDOCFLAGS = '-D warnings'
cargo doc --workspace --locked --no-deps
~~~

## GPUI and interaction

Keep audio and device recovery on the engine side. Bind keyboard and pointer activation to the same settings operation, give each interactive element a stable ID, and keep focus visible. Show mute and processing state in text. Pair the level meter with a numeric or silence readout. Check actions against the examples for the version recorded in Cargo.lock. [GPUI examples](https://gpui.rs/examples/) · [Windows keyboard accessibility](https://learn.microsoft.com/en-us/windows/apps/design/accessibility/keyboard-accessibility) · [Windows accessibility checklist](https://learn.microsoft.com/en-us/windows/apps/design/accessibility/accessibility-checklist)

Optional agent references for this project are Apollo's [Rust best practices](https://github.com/apollographql/skills/tree/9dda1397730370aeb8df4ff8f2015c34c86b5003/skills/rust-best-practices) and Longbridge's [GPUI action skill](https://github.com/longbridge/gpui-component/tree/8671be20052e30a92ad555bbe384eabb32446549/.claude/skills/gpui-action). Both cover patterns used in the codebase. The pinned GPUI crate examples serve as the API reference.
