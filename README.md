# Orvia

Orvia turns one Windows microphone into a processed recording device that another app can select. Its Rust engine captures WASAPI input, applies channel mixing, optional RNNoise suppression, gain and mute, then feeds the Orvia WaveRT driver. The GPUI settings window can close while the engine keeps running.

**Development build for Windows 11 x64.** The installer includes Orvia's own test-signed driver. Loading it requires a Windows test environment prepared for that signature. A stock Secure Boot installation cannot load this build. The project has no production driver signature or v1.0 release.

## Use in a test environment

1. Prepare a Windows test machine and trust the public test certificate that accompanies your build. Follow the [driver instructions](driver/README.md).
2. Run the matching `Orvia-<version>-windows-x64-test.msi`, then open **Orvia** from Start.
3. Select a microphone. **Stereo** preserves its first two channels. **Average L + R**, **Left only** and **Right only** send the selected signal to both output channels. A single-channel input automatically feeds both sides.
4. Select the **Orvia Audio v1** recording endpoint in your call or recording app. The settings window shows the engine state, output peak, and dropped or missing frames.
5. Adjust gain, mute or noise suppression as needed. Closing the window keeps the engine running. Reopen it from Start. The engine also starts at user sign-in.

Check for updates in the settings window. While the repository is private, **Open releases** uses your browser's GitHub session. Install a newer MSI over the previous version. Uninstall **Orvia** through Windows Installed apps. Preferences remain in `%LOCALAPPDATA%\Orvia` for a later installation.

## Build

Install Rust 1.93.1 with the x64 MSVC toolchain, Visual Studio 2022 C++ Build Tools, and a Windows SDK. From the repository root run the following commands.

~~~powershell
cargo build --workspace --locked
cargo test --workspace --locked
cargo doc --workspace --no-deps --open
cargo run -p orvia-engine --example inputs
.\target\debug\orvia.exe
~~~

The input diagnostic lists device names and formats without opening an audio stream. The settings window and tests can run before the driver is installed. See [packaging and verification](docs/testing.md) for the installer, test coverage and release conditions.

## Source and licenses

- [Audio processing and configuration](crates/core/src/lib.rs) are shared by the settings window and engine.
- [WASAPI transport](crates/engine/src/stream.rs) negotiates rates and manages the bounded audio queue.
- [GPUI settings](crates/settings/src/main.rs) use keyboard actions and display engine observations.
- [Driver source](driver/) exposes the capture and render pins under one stable device identity.
- [Contributing](CONTRIBUTING.md) covers API documentation, review and UI conventions.

The Rust application and installer use the [MIT License](LICENSE). The driver directory uses the [Microsoft Public License](driver/LICENSE). Packaging generates notices for the Windows dependency graph, including GPUI and the embedded denoiser.
