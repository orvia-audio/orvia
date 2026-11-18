# Packaging and verifying Orvia

## Build an installer

Use an x64 Native Tools shell. Install the Rust prerequisites from the README and put NuGet CLI, WiX 4.0.6 (`wix`) and cargo-about 0.9.3 (`cargo-about`) on PATH. The WiX .NET tool also requires a compatible .NET runtime. The Windows workflow builds the same package from a clean checkout.

~~~bat
nuget install Microsoft.Windows.WDK.x64 -Version 10.0.26100.6584 -OutputDirectory target\wdk -Source https://api.nuget.org/v3/index.json -NonInteractive
powershell -NoProfile -File driver\build.ps1 -TestSign
powershell -NoProfile -File installer\build.ps1
~~~

The `dist` directory contains the x64 MSI, public test certificate, third-party notices and SHA-256 checksums. Compare the MSI and certificate hashes with the values from the same build. The private key is never part of the package. The installer needs administrator rights to manage the device. The engine and settings window run in the signed-in user's session. Open Orvia from Start after installing.

The installer owns `ROOT\ORVIAAUDIO\0000` and its driver package. A major upgrade keeps the device node and the per-user settings. App, driver and PCM transport versions are independent. Keep the adapter name and capture pin reference stable for compatible changes. A change to the transport format needs a new protocol identity and an engine compatibility decision.

## Automated checks

The Windows workflow checks formatting, Clippy, Rustdoc links and missing documentation, unit tests and doctests. It builds and test-signs the x64 driver, checks the INF, generates a catalog, and packages development artifacts. It does not publish a GitHub Release.

The deterministic audio tests cover channel selection, clipping, fail-silent mute, stale queue recovery and resampling counts. Capture supports float shared formats from 8 to 48 kHz. The engine chooses 48 kHz when the device advertises it. For multichannel interfaces it uses the first two channels. The driver uses a 48 kHz stereo PCM stream and the Windows audio service handles conversion at its WASAPI endpoints.

## Verification on a test machine

Run the exact MSI and matching certificate produced by the build. Keep measurements with the build commit, Windows build, endpoint formats, application versions and boot policy. Record each item independently.

| Check | Procedure and evidence |
| --- | --- |
| First install | Install on a clean test-signing machine. Open settings as a standard user. Record the capture endpoint ID and check that the default speakers remain usable. |
| Delivery | Record speech in Windows Sound Recorder, then in a browser call and a desktop calling app. Check left, right and average with distinct signals on each input. Open two consumers at once. |
| Quality | Compare a fixed speech-and-noise sample with suppression off and on. Listen for clipped consonants, pumping and switch noise. Inspect clipping at 100% and 200% gain. |
| Recovery | Unplug the selected microphone, disable and re-enable it, sleep and resume, then restart the engine. Verify silence during loss and no buffered replay. Repeat with mute enabled. |
| Timing | During an extended run and a separate high-load run, log elapsed time, missing and dropped frames, CPU utilization and loopback impulse delay. Establish pass thresholds from measured baseline data. |
| Input diversity | Cover a single-channel microphone, a stereo or multichannel USB interface, and a wireless headset. The Steinberg UR12 and Razer Barracuda X are example devices. Record the format Windows actually negotiates. |
| Accessibility | Operate every control using Tab, Shift+Tab, Space and Enter. Check focus visibility, Windows contrast themes, 200% scaling and Narrator. |
| Upgrade and removal | Change input, mute and gain. Install a newer MSI and compare the endpoint ID and settings. Uninstall and verify the device, startup entry and executables are removed. |
| Boot policy | A test signature requires a prepared test system. A production-ready release for ordinary Secure Boot installations would need a separately authorized signing path and clean-machine testing. |

### Verify mute with an independent recording

Feed a continuous tone to the selected input. Set gain to 100% and turn off noise suppression. Start a WASAPI recording of **Microphone Array (Orvia Audio v1)** and a second recording of the source input. Keep both recorders open while clicking **Mute microphone**, waiting at least six seconds, and clicking **Unmute microphone**. Check that the tone continues in the reference recording throughout. Inspect a settled interval in each phase of the Orvia recording for the original tone, digital silence and the return of the tone.

On Windows 11 Enterprise LTSC 2024 x64 (build 26100.1742), a QEMU line input carried a 440 Hz and 880 Hz tone. The reference recorder used 44.1 kHz and the Orvia recorder used 48 kHz. The source measured −19.1 dBFS RMS in all three intervals. Orvia measured −19.1 dBFS before and after mute. All 168,000 samples in the 3.5-second muted interval were zero in the recorded channel. This test covers the installed driver, audio engine and settings buttons with an emulated input. Hardware compatibility and sustained audio quality need separate measurements.

## Troubleshooting

- **The adapter appears but its microphone is missing after reinstalling.** Close audio applications. Check the latest Orvia installation entry in `%WINDIR%\inf\setupapi.dev.log`. If Windows reports that the driver update requires a reboot, restart the test machine before opening the recorder. The test MSI can report success while that reboot is pending.

- **Orvia Audio v1 is unavailable.** Check Device Manager for Orvia's Media device. Compare the public certificate with the one used for the installed package. See the [driver guide](../driver/README.md). The engine retries when the adapter appears.
- **The selected microphone is disconnected.** Reconnect it or choose another input. An explicit selection is retained by endpoint ID. Choosing the Windows default follows later default changes.
- **Only one side carries speech.** Choose Left only or Right only for an interface with a microphone on one input.
- **No compatible shared format.** Check microphone access for desktop apps in Windows Privacy settings and try a 48 kHz shared format under Sound settings.
- **No public release is available.** The anonymous GitHub lookup cannot see private releases. Open releases in a signed-in browser.

When reporting a problem, include the application version, driver version from Device Manager, Windows build, selected format, status text and changes in the frame counters. While settings is open, `%LOCALAPPDATA%\Orvia\status.json` contains a timestamped snapshot. Neither the snapshot nor the normal driver path records microphone audio.
