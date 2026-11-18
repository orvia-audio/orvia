# Orvia

Orvia sends one microphone through channel mixing, optional RNNoise suppression, gain and mute to a virtual audio device. A game or recorder can use the processed signal while a call uses the original microphone. Closing the settings window leaves the audio engine running.

**Windows 11 x64 with Secure Boot enabled.** The standard MSI installs the application. Install a supported, production-signed virtual audio device separately. The private development builds are available as artifacts of the Windows workflow.

## Route a microphone

1. Install a virtual cable. Eugene Muzychenko provides **Virtual Audio Cable Lite** for private home use unrelated to income generation. Its single cable exposes a playback device and a recording device, both usually called **Line 1 (Virtual Audio Cable)**. [VAC downloads and license](https://vac.muzychenko.net/en/download.htm)
2. Install `Orvia-<version>-windows-x64.msi`, then open **Orvia** from Start.
3. Under **Input**, select your physical microphone. Under **Virtual microphone output**, select the cable's playback device. For a stereo interface with a microphone on only one channel, choose **Left only** or **Right only** to send it to both sides.
4. Select the cable's recording device in each game voice client or recorder that should receive the processed signal. Select the physical microphone explicitly in apps such as Discord that should receive the original signal. This setup works with the existing Windows defaults.
5. Turn on **Noise suppression**, adjust **Output gain**, or mute in Orvia. These controls affect only the virtual output. Orvia remembers the endpoint IDs and waits for a disconnected device to return.

With **Elgato Wave Link**, select a dedicated playback channel on the **Elgato Virtual Audio** adapter. Route only that channel to a mix in Wave Link and select that mix as the recording device in the receiving apps. [Wave Link audio routing](https://www.elgato.com/us/en/explorer/products/wave/wave-link-30-redefining-creator-audio-control/)

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

The Rust application and installer use the [MIT License](LICENSE). The optional [lab driver](driver/README.md) uses the [Microsoft Public License](driver/LICENSE). Packaging generates notices for the Windows dependency graph, including GPUI and the embedded denoiser.
