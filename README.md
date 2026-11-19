# <img src="assets/orvia-wordmark.svg" alt="Orvia" width="560">

Give your game or recorder a cleaner microphone while your voice chat keeps the original. Orvia removes background noise on your own CPU. Listen through headphones, watch the before-and-after signal, and adjust the gain while you speak.

![Orvia's mixer showing DeepFilterNet3, the live signal history, the gain slider and headphone monitoring](assets/mixer.png)

## Get started

| System | Install | Microphone for your game or recorder |
| --- | --- | --- |
| Windows 11 x64 | Install the [MSI](https://github.com/sya-ri/orvia/actions/workflows/windows.yml) and a supported virtual cable. Secure Boot can stay on. | The recording side of [Virtual Audio Cable](https://vac.muzychenko.net/en/download.htm) or an Elgato Wave Link mix |
| macOS 13+ | Open the [DMG for your Mac](https://github.com/sya-ri/orvia/actions/workflows/desktop.yml), drag Orvia to Applications and install [BlackHole 2ch](https://github.com/ExistentialAudio/BlackHole). Allow microphone access when prompted. | BlackHole 2ch |
| Ubuntu 24.04 x64 with PipeWire | Install the [DEB](https://github.com/sya-ri/orvia/actions/workflows/desktop.yml) and sign out and back in to load its virtual microphone. | Orvia Microphone |

In **Devices**, select your physical microphone and the matching virtual output. Choose a filter in **Mixer**. In apps that should hear the original, keep the physical microphone selected.

[Build and verify](docs/testing.md) · [Contributing](CONTRIBUTING.md) · [MIT license](LICENSE)
