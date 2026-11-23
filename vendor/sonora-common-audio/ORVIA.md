# Orvia integration

Based on [sonora-common-audio 0.3.0](https://github.com/dignifiedquire/sonora/tree/1632d47c1e77d0857f9145bd175134b348541e11). The library sources and license come from the published Cargo package.

Orvia can transfer the streaming history of the biquad and sinc resampler between identically configured channels. The destination reuses its coefficient tables and buffers.

Fixed-frame push resamplers keep only the active convolution kernel. The adjustable pull resampler precomputes its lookup tables for live ratio changes.
