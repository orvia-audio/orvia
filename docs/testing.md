# Packaging and verifying Orvia

## Build an installer

Use an x64 Native Tools shell. Install the Rust prerequisites from the README and put WiX 4.0.6 (`wix`) and cargo-about 0.9.3 (`cargo-about`) on PATH. The WiX .NET tool also requires a compatible .NET runtime. Run `powershell -NoProfile -File installer\build.ps1`. The standard MSI, third-party notices and SHA-256 checksums appear in `dist`. This package needs no WDK, driver package or certificate and leaves third-party virtual devices under their own installer's control.

For the optional lab edition, also put NuGet CLI on PATH and run the following commands.

~~~bat
nuget install Microsoft.Windows.WDK.x64 -Version 10.0.26100.6584 -OutputDirectory target\wdk -Source https://api.nuget.org/v3/index.json -NonInteractive
powershell -NoProfile -File driver\build.ps1 -TestSign
powershell -NoProfile -File installer\build.ps1 -WithTestDriver
~~~

The lab artifacts are in `dist/lab`. Compare the MSI and public test certificate with the hashes from the same build. Both editions run the engine and settings window in the signed-in user's session. Uninstall the current edition before switching between them. The per-user settings survive removal.

The lab installer owns `ROOT\ORVIAAUDIO\0000` and its driver package. A lab upgrade keeps the device node and the per-user settings. Existing settings without an output ID continue to use Orvia Audio v1 when it is available.

## Automated checks

The Windows workflow checks formatting, Clippy, Rustdoc links and missing documentation, unit tests and doctests. It builds and test-signs the x64 driver, checks the INF, generates a catalog, and packages development artifacts. It does not publish a GitHub Release.

The deterministic audio tests cover channel selection, suppression of stationary noise, gain, clipping, mute, stale queue recovery and rate conversion. Capture supports float shared formats from 8 to 48 kHz. For multichannel interfaces it uses the first two channels. The output supports mono or stereo float or 16-bit PCM from 8 to 48 kHz. The processor runs at 48 kHz regardless of the negotiated endpoint rates.

## Verification on a test machine

For the standard edition, use a Secure Boot system and a vendor-signed cable. Compare simultaneous captures of the physical microphone and the cable while varying suppression, gain and mute. Check that the Windows defaults remain unchanged and that disabling the selected cable stops output until it returns. The lab edition instead requires its matching test certificate. Keep measurements with the build commit, Windows build, endpoint formats, application versions and boot policy.

| Check | Procedure and evidence |
| --- | --- |
| First install | Install on a clean machine with the appropriate boot policy for the edition. Open settings as a standard user. Record the capture endpoint ID and check that the default speakers remain usable. |
| Delivery | Record speech in Windows Sound Recorder, then in a browser call and a desktop calling app. Check left, right and average with distinct signals on each input. Open two consumers at once. |
| Quality | Compare a fixed speech-and-noise sample with suppression off and on. Listen for clipped consonants, pumping and switch noise. Inspect clipping at 100% and 200% gain. |
| Recovery | Unplug the selected microphone, disable and re-enable it, sleep and resume, then restart the engine. Verify silence during loss and no buffered replay. Repeat with mute enabled. |
| Timing | During an extended run and a separate high-load run, log elapsed time, missing and dropped frames, CPU utilization and loopback impulse delay. Establish pass thresholds from measured baseline data. |
| Input diversity | Cover a single-channel microphone, a stereo or multichannel USB interface, and a wireless headset. The Steinberg UR12 and Razer Barracuda X are example devices. Record the format Windows actually negotiates. |
| Accessibility | Operate every control using Tab, Shift+Tab, Space and Enter. Check focus visibility, Windows contrast themes, 200% scaling and Narrator. |
| Upgrade and removal | Change input, output, mute and gain. Install a newer MSI and compare the endpoint IDs and settings. Uninstall and check that the startup entry and executables are removed. A third-party cable belongs to its vendor. Only the lab MSI removes its own device. |
| Boot policy | Verify that the standard MSI works with Secure Boot enabled and test signing off. Use the lab MSI only in an environment prepared for its test signature. |

### Verify mute with an independent recording

Feed a continuous tone to the selected input. Set gain to 100% and turn off noise suppression. Start a WASAPI recording of **Microphone Array (Orvia Audio v1)** and a second recording of the source input. Keep both recorders open while clicking **Mute microphone**, waiting at least six seconds, and clicking **Unmute microphone**. Check that the tone continues in the reference recording throughout. Inspect a settled interval in each phase of the Orvia recording for the original tone, digital silence and the return of the tone.

On Windows 11 Enterprise LTSC 2024 x64 (build 26100.1742), a QEMU line input carried a 440 Hz and 880 Hz tone. The reference recorder used 44.1 kHz and the Orvia recorder used 48 kHz. The source measured −19.1 dBFS RMS in all three intervals. Orvia measured −19.1 dBFS before and after mute. All 168,000 samples in the 3.5-second muted interval were zero in the recorded channel. This test covers the installed driver, audio engine and settings buttons with an emulated input. Hardware compatibility and sustained audio quality need separate measurements.

## Troubleshooting

- **The adapter appears but its microphone is missing after reinstalling.** Close audio applications. Check the latest Orvia installation entry in `%WINDIR%\inf\setupapi.dev.log`. If Windows reports that the driver update requires a reboot, restart the test machine before opening the recorder. The test MSI can report success while that reboot is pending.

- **No virtual output found.** Install a supported virtual audio device, enable its playback endpoint in Sound settings, then reopen the output picker. Both sides of Virtual Audio Cable normally have the same name. Choose its playback side in Orvia and its recording side in downstream apps.
- **Selected virtual output is disconnected.** Reconnect or enable the device. Orvia retries the saved endpoint and never falls back to the default speakers. If reinstalling a cable changes its endpoint ID, select it again.
- **The selected microphone is disconnected.** Reconnect it or choose another input. An explicit selection is retained by endpoint ID. Choosing the Windows default follows later default changes.
- **Only one side carries speech.** Choose Left only or Right only for an interface with a microphone on one input.
- **No compatible shared format.** Check microphone access for desktop apps in Windows Privacy settings and try a 48 kHz shared format under Sound settings.
- **No public release is available.** The anonymous GitHub lookup cannot see private releases. Open releases in a signed-in browser.

When reporting a problem, include the application version, driver version from Device Manager, Windows build, selected format, status text and changes in the frame counters. While settings is open, `%LOCALAPPDATA%\Orvia\status.json` contains a timestamped snapshot. Neither the snapshot nor the normal driver path records microphone audio.
