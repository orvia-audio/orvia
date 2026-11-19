# <img src="assets/orvia.svg" alt="" width="40" height="40" align="center"> Orvia

Orvia sends one microphone through channel mixing, optional noise suppression, gain and mute to a virtual audio device. A game or recorder can use the processed signal while a call uses the original microphone. Closing the settings window leaves the audio engine running.

**Windows 11 x64 with Secure Boot enabled.** The standard MSI installs the application. Install a supported, production-signed virtual audio device separately. The private development builds are available as artifacts of the Windows workflow.

Development builds for the first release carry versions such as `0.1.0-snapshot.1`. The first stable release will be `v0.1.0`. Preferences and the installer name show the application version. Windows lists a separate numeric version for installer upgrades.

## Route a microphone

1. Install a virtual cable. Eugene Muzychenko provides **Virtual Audio Cable Lite** for private home use unrelated to income generation. Its single cable exposes a playback device and a recording device, both usually called **Line 1 (Virtual Audio Cable)**. [VAC downloads and license](https://vac.muzychenko.net/en/download.htm)
2. Install `Orvia-<version>-windows-x64.msi`, then open **Orvia** from Start.
3. Open **Devices**. Under **Microphone input**, select your physical microphone. Under **Virtual microphone output**, select the cable's playback device. For a stereo interface with a microphone on only one channel, choose **Mono** in the **Mixer** to send the average of both channels to both sides.
4. Select the cable's recording device in each game voice client or recorder that should receive the processed signal. Select the physical microphone explicitly in apps such as Discord that should receive the original signal. This setup works with the existing Windows defaults.
5. In **Mixer**, select **RNNoise** for speech, **Spectral subtraction** for steady background sounds such as fans, or **Off** to bypass suppression. Drag **Output gain** from 0% to 200% or use the arrow keys on the slider. Apps that capture the physical microphone directly keep their original audio.

**Signal history** shows two seconds of peak envelopes before and after suppression on the same dBFS scale. Bypass shows only the input trace. Switching filters keeps the timeline running. **Pre-gain attenuation** compares the signal energy over the latest half second. It includes any change to speech as well as noise. Pause speaking to measure the background alone. The output reading includes gain, so you can adjust recording volume independently.

To hear the same processing, select a playback device under **Mixer → Headphone monitor**. Use the monitor switch to listen or stop listening. Orvia remembers the selected headphones and reconnects when they return. Mute in the notification area silences both processed outputs.

Use **Preferences → Audio engine → Restart** to reload the installed engine after an update. This briefly interrupts processed audio. **View log** shows engine starts, stops, device errors and changes to processing as they happen. The 160 most recent events survive a restart and carry local timestamps. The log contains no audio samples.

Use **Preferences → Language** to follow Windows or choose English or Japanese. Closing the window leaves Orvia in the notification area. Its menu opens the mixer, mutes or unmutes the processed outputs, and quits the engine.

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

The interface uses outline SVGs from [Tabler Icons v3.52.0](https://github.com/tabler/tabler-icons/tree/v3.52.0/icons/outline) under the [MIT License](assets/tabler/LICENSE).
