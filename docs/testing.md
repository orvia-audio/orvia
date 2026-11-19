# Build and verify Orvia

Use Rust 1.93.1. Build both executables with `cargo build --workspace --locked` and launch `target/debug/orvia` (`orvia.exe` on Windows). The mixer starts the engine beside it.

| Target | Build requirements | Package |
| --- | --- | --- |
| Windows x64 | MSVC 2022 C++ tools, Windows SDK, WiX 4.0.6 and cargo-about 0.9.3 | Run `installer/build.ps1` from an x64 Native Tools shell. |
| macOS | Xcode Command Line Tools, jq and cargo-about 0.9.3 | Run `MACOSX_DEPLOYMENT_TARGET=13.0 bash installer/desktop.sh` for a native disk image. |
| Ubuntu 24.04 amd64 | Install the packages listed in the [desktop workflow](../.github/workflows/desktop.yml), plus jq and cargo-about 0.9.3. | Run `bash installer/desktop.sh` for a DEB with a PipeWire virtual sink and capture source. |

Find native packages in `dist/`. The macOS app is ad-hoc signed. If Gatekeeper blocks the first launch, review the app under **System Settings → Privacy & Security**. Quit Orvia from its menu before replacing or removing the macOS or Linux app. Windows Installer resumes an active engine after an upgrade or repair.

## Audio routing checks

Open **Mic test**, choose the same virtual microphone as your receiving app, and select your headphones. Select **Before update** and click **Record**. Speak, pause briefly, then speak again. Stop or wait ten seconds. The same button now reads **Play recording**. Use **Retake** to record another take. After upgrading, repeat under **After update** and switch between the two recordings to compare them.

To test an older installation, run the newer mixer with `--mic-test`. This opens only the recorder and shows the running engine version. Both recordings are stored locally as `mic-test/before.wav` and `mic-test/after.wav` in Orvia's user data directory. Retaking replaces only the selected recording.

1. Select a physical input and a virtual output. Record from both devices while moving gain from 100% to 0% and back. Only the virtual recording should change.
2. Switch between Off and the three filters. Check that the mixer keeps its history and that the headphones follow the processed output.
3. Unplug and reconnect the input, output and headphones in turn. The engine should recover the selected route without sending audio to a fallback speaker.

On Windows, Secure Boot can stay enabled with the signed virtual cables. **VAC Lite** is licensed for private home use unrelated to income generation. A dedicated Elgato Wave Link playback channel can also feed a recording mix. The optional [Windows lab driver](../driver/README.md) has separate signing requirements.

On macOS, drag Orvia from the DMG to Applications. Grant microphone access on first use and choose **BlackHole 2ch** as the virtual output in Orvia and as the input in the receiving app. On Ubuntu, choose **Orvia Output** in the mixer and **Orvia Microphone** in the receiver. The Linux integration test records a known tone through that route in an isolated PipeWire session. See **Preferences → Audio engine → View log** for connection errors.
