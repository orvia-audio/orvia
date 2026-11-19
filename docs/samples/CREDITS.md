# Listening samples

| Recording | Credit | Terms |
| --- | --- | --- |
| [Speech 12dB s16](https://commons.wikimedia.org/wiki/File:Speech_12dB_s16.flac) | IETF / Opus Codec working group, [Opus examples](https://opus-codec.org/examples/) | [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/) |
| [Keyboard noise](https://commons.wikimedia.org/wiki/File:Keyboard_noise.ogg) | Yuyudevil | Public domain |
| [Air conditioner hum](https://commons.wikimedia.org/wiki/File:Air_conditioner_hum_(Gravity_Sound).wav) | Gravity Sound | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |

Each 14-second mix uses the full 10.8-second speech recording starting at 2 seconds. The backgrounds were downmixed to mono, resampled to 48 kHz, trimmed to 14 seconds and centered to remove DC offset. Noise was set 5 dB below the speech excerpt by RMS. One common gain per mix set its peak to 0.8 full scale.

Orvia processed each mix once per filter, in mono at 100% gain. The lossless clips use 16-bit FLAC. Filter delay was removed by cross-correlation with clean speech. All eight FLACs retain the same level and duration. The 16-second H.264/AAC video repeats the 0.70–4.70 second excerpt of the air conditioner mix in four versions. Each waveform shows the corresponding audio at the same scale.

| Filter | Typing reduction | Typing STOI | Air conditioner reduction | Air conditioner STOI |
| --- | ---: | ---: | ---: | ---: |
| Off | 0 dB | 0.783 | 0 dB | 0.793 |
| DeepFilterNet3 | 28.2 dB | 0.916 | 40.6 dB | 0.902 |
| RNNoise | 39.9 dB | 0.851 | 54.8 dB | 0.847 |
| WebRTC High | 23.1 dB | 0.771 | 17.7 dB | 0.777 |

Reduction compares RMS over 1.0–2.0 seconds, with background alone. [Short-time objective intelligibility (STOI)](https://doi.org/10.1109/TASL.2011.2114881) compares the speech passage to its clean reference. Higher is better. Both measurements describe these mixes.
