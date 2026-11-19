# <img src="assets/orvia-wordmark.svg" alt="Orvia" width="560">

Orvia is a desktop microphone mixer for Windows, macOS, and Linux. Reduce background noise and adjust gain locally, then choose which apps receive the original mic or the processed mix.

![Orvia mixer with live input and filtered signal levels, channel controls, output gain, and headphone monitoring](assets/mixer.png)

## Features

- **Routing.** Connect a physical input to a virtual microphone that each app can select independently.
- **Mixing.** Keep stereo channels separate or mix them to mono. Adjust output gain from 0% to 200%.
- **Noise suppression.** Process speech locally with DeepFilterNet3, RNNoise, or WebRTC.
- **Monitoring.** Listen to the processed signal through your headphones and watch input and filtered levels in real time.

https://github.com/user-attachments/assets/f70d9515-6fc0-45b6-9218-495529a97bc9

[Compare filters on the same recording](https://gh.s7a.dev/orvia/#listen)

## Getting started

[Download Orvia from GitHub Releases](https://github.com/sya-ri/orvia/releases/latest)

| System | Installer | Virtual microphone |
| --- | --- | --- |
| Windows 11 x64 | MSI | [Virtual Audio Cable](https://vac.muzychenko.net/en/download.htm) or an Elgato Wave Link mix |
| macOS 13+ | DMG for Apple Silicon or Intel | [BlackHole 2ch](https://github.com/ExistentialAudio/BlackHole) |
| Ubuntu 24.04 x64 with PipeWire | DEB | Orvia Microphone, included in the package |

1. Install Orvia and the virtual audio device for your system. On Ubuntu, sign out and back in after installation.
2. In **Devices**, choose your physical microphone and virtual output. In **Mixer**, set the gain and turn on headphone monitoring if you want to hear the result.
3. In each app's audio settings, select the physical microphone for the original audio or the virtual microphone for the Orvia mix.

Orvia checks for a new stable release when you open the mixer and once a day while it stays open. Select the update notice to open the release. You can turn off automatic checks in **Preferences**.

## Development

- [Build and test Orvia](docs/testing.md)
- [Contributing guidelines](CONTRIBUTING.md)
