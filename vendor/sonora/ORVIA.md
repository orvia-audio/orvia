# Orvia integration

Based on [sonora 0.3.0](https://github.com/dignifiedquire/sonora/tree/1632d47c1e77d0857f9145bd175134b348541e11). The library sources and licenses come from the published Cargo package. This distribution builds the library target.

Orvia adds a fixed 48 kHz mono noise-only channel with the upstream High preset. Two matching channels can share its output until their inputs diverge. The stream-copy operation then transfers the resampler, filter-bank, high-pass and noise-estimator history. The capture pipeline otherwise follows upstream.
