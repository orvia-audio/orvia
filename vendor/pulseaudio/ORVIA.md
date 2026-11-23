# PulseAudio client

This directory contains the library and examples from the MIT-licensed
[`pulseaudio` 0.3.2](https://crates.io/crates/pulseaudio/0.3.2).
The archive SHA-256 is `9ba79a4c6904c987cc31fa50e9b750539404511c2dbd9ce95e5e58444177a21e`.

Orvia sets a two-second socket I/O timeout before the desktop client's
blocking authentication handshake. A silent server releases the worker so a
later attempt can connect. Streaming uses the upstream nonblocking reactor.
