// Orvia's v1 transport carries 48 kHz stereo signed 16-bit PCM.
#pragma once

namespace OrviaBridge {
void Initialize();
void Reset();
void Protect(BOOL protectedContent);
void Render(const BYTE* data, ULONG bytes);
void Capture(BYTE* data, ULONG bytes);
}
