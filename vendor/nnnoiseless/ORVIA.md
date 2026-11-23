# Orvia integration

Based on [nnnoiseless 0.5.2](https://github.com/jneem/nnnoiseless/tree/924a2dd143ccad7bce9e5bda061b60ca32911a67). The library sources and licenses come from the published Cargo package. This distribution builds the library target.

Orvia adds an allocation-free history transfer for the recurrent states, synthesis overlap, and pitch detector. The model weights and inference algorithm match upstream.

For exact digital silence, the library reuses a frame only after the high-pass, pitch and input histories reach a bit-identical fixed point and inference skips the frame. The synthesis overlap still contributes to the next output. The workspace tests compare silence transitions and resumed speech with the published crate.

On x86-64 CPUs with AVX2, contiguous GRU gate rows share input loads. Each neuron retains its scalar accumulation order. Other CPUs use the portable inference path.
