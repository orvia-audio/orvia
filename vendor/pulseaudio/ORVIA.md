# PulseAudio client

This directory contains the library and examples from the MIT-licensed
[`pulseaudio` 0.3.2](https://crates.io/crates/pulseaudio/0.3.2).
The archive SHA-256 is `9ba79a4c6904c987cc31fa50e9b750539404511c2dbd9ce95e5e58444177a21e`.

Orvia sets a two-second socket I/O timeout before the desktop client's
blocking authentication handshake. A silent server releases the worker so a
later attempt can connect.

After authentication, the reactor waits up to five seconds for a control
reply. If the server stops answering, it closes the connection and releases
pending device queries and stream shutdown requests. With no control request
pending, the reactor sleeps until the socket or its waker becomes ready.
Playback drains can wait for the listener to resume a paused stream.
Dropping the last client wakes the reactor so even an idle device scan
releases its worker thread and socket.
