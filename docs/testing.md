# Build and verify Orvia

Use Rust 1.93.1. Build both executables with `cargo build --workspace --locked` and launch `target/debug/orvia` (`orvia.exe` on Windows). The mixer starts the engine beside it.

| Target | Build requirements | Package |
| --- | --- | --- |
| Windows x64 | MSVC 2022 C++ tools, Windows SDK, WiX 4.0.6 and cargo-about 0.9.3 | Run `installer/build.ps1` from an x64 Native Tools shell. |
| macOS | Xcode Command Line Tools, jq and cargo-about 0.9.3 | Run `MACOSX_DEPLOYMENT_TARGET=13.0 bash installer/desktop.sh` for a native disk image. |
| Ubuntu 24.04 amd64 | Install the packages listed in the [desktop workflow](../.github/workflows/desktop.yml), plus jq and cargo-about 0.9.3. | Run `bash installer/desktop.sh` for a DEB with a PipeWire virtual sink and capture source. |

Find native packages in `dist/`. The macOS app is ad-hoc signed. If Gatekeeper blocks the first launch, review the app under **System Settings → Privacy & Security**. Quit Orvia from its menu before replacing or removing the macOS or Linux app. Windows Installer resumes an active engine after an upgrade or repair.

## Audio routing checks

Under **Mixer → Tone → Auto setup**, start recording and make the low sound you want to reduce. Select **Next** and read the numbers at your usual distance. During capture, the engine bypasses low cut and compression and sets gain to 100%. The selected noise filter stays active if enabled. Listen to **Current** and **Recommended** on the same receiver recording, then choose **Apply settings** to save all three controls.

Under **Mixer → DeepFilterNet3**, choose **Auto adjust**. Stay quiet for the room sample. When the numbers appear, say a few words at your usual distance. Each take ends after the sound returns near room level for 1.5 seconds. Repeat at the next prompt. For the final reading it temporarily auditions the suggested limit, records the receiving endpoint, restores the saved limit and plays what the virtual microphone received. Choose **Apply** to keep the suggestion.

The suggestion aims for 9 dB of room noise reduction while keeping the first 150 ms of each phrase within 6 dB of the original in active 50 ms windows. The audition recording stays in memory for this session.

The headphone monitor listens to the receiving side of the selected virtual cable. If matching is ambiguous, set **Devices → Virtual microphone recording endpoint**.

Open **Mic test**, choose the same virtual microphone as your receiving app, and select your headphones. Click **Record** and start speaking when the numbers appear. You can stop speaking at any number or use your own words. Click the same button to play the recording. Choose **Retake** to replace it.

To test an older installation, run the newer mixer with `--mic-test`. This opens the recorder and shows the running engine version. The latest take is saved as `mic-test/recording.wav` in Orvia's user data directory. To compare across updates, copy that file before retaking.

1. Select a physical input and a virtual output. Record from both devices while moving gain from 100% to 0% and back. Only the virtual recording should change.
2. Switch between Off and the three filters. Check that the mixer keeps its history and that the headphones follow the processed output.
3. Unplug and reconnect the input, output and headphones in turn. The engine should recover the selected route without sending audio to a fallback speaker.

On Windows, Secure Boot can stay enabled with the signed virtual cables. **VAC Lite** is licensed for private home use unrelated to income generation. A dedicated Elgato Wave Link playback channel can also feed a recording mix. The optional [Windows lab driver](../driver/README.md) has separate signing requirements.

On macOS, drag Orvia from the DMG to Applications. Grant microphone access on first use and choose **BlackHole 2ch** as the virtual output in Orvia and as the input in the receiving app. On Ubuntu, choose **Orvia Output** in the mixer and **Orvia Microphone** in the receiver. The Linux integration test records a known tone through that route in an isolated PipeWire session. Open **Preferences → Audio engine → Open log file** to read connection errors in your text editor. Each line includes the timestamp, severity and event.
