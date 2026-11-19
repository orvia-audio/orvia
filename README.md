# <img src="assets/orvia-wordmark.svg" alt="Orvia" width="560">

Orvia is a desktop microphone mixer for Windows, macOS, and Linux. Send a processed microphone to a game or recording app while your voice chat uses the original input.

![Orvia mixer with live input and filtered signal levels, channel controls, output gain, and headphone monitoring](assets/mixer.png)

## Features

- **Routing.** Connect a physical input to a virtual microphone that each app can select independently.
- **Mixing.** Keep stereo channels separate or mix them to mono. Adjust output gain from 0% to 200%.
- **Noise suppression.** Process speech locally with DeepFilterNet3, RNNoise, or WebRTC.
- **Monitoring.** Listen to the processed signal through your headphones and watch input and filtered levels in real time.

https://github.com/user-attachments/assets/f70d9515-6fc0-45b6-9218-495529a97bc9

## Getting started

| System | Snapshot installer | Virtual microphone |
| --- | --- | --- |
| Windows 11 x64 | [MSI](https://github.com/sya-ri/orvia/actions/workflows/windows.yml) | [Virtual Audio Cable](https://vac.muzychenko.net/en/download.htm) or an Elgato Wave Link mix |
| macOS 13+ | [DMG for Apple Silicon or Intel](https://github.com/sya-ri/orvia/actions/workflows/desktop.yml) | [BlackHole 2ch](https://github.com/ExistentialAudio/BlackHole) |
| Ubuntu 24.04 x64 with PipeWire | [DEB](https://github.com/sya-ri/orvia/actions/workflows/desktop.yml) | Orvia Microphone, included in the package |

1. Install Orvia and the virtual audio device for your system. On Ubuntu, sign out and back in after installation.
2. In **Devices**, choose your physical microphone and virtual output. In **Mixer**, set the gain and turn on headphone monitoring if you want to hear the result.
3. In your game or recorder, select the virtual microphone. Leave your physical input selected in apps that should receive the original sound.

[Listening samples and A/B player](docs/samples/) · [Build and test](docs/testing.md) · [Contributing](CONTRIBUTING.md) · [MIT license](LICENSE)
