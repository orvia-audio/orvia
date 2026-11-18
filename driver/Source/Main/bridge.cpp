// The adapter factory permits one live adapter. Both WaveRT pins share this
// nonpaged queue. The position lock can nest outside the queue lock.
#include "definitions.h"
#include "bridge.h"

namespace {
constexpr ULONG Capacity = 4800 * 4;
constexpr ULONGLONG MaxAge = 100 * 10000; // 100 ms in interrupt-time units.
KSPIN_LOCK lock;
BYTE samples[Capacity];
ULONG head, count;
ULONGLONG lastRender;
BOOL protectedStream;
}

void OrviaBridge::Initialize()
{
    KeInitializeSpinLock(&lock);
}

void OrviaBridge::Reset()
{
    KIRQL irql;
    KeAcquireSpinLock(&lock, &irql);
    head = count = 0;
    KeReleaseSpinLock(&lock, irql);
}

void OrviaBridge::Protect(BOOL protectedContent)
{
    KIRQL irql;
    KeAcquireSpinLock(&lock, &irql);
    protectedStream = protectedContent;
    head = count = 0;
    KeReleaseSpinLock(&lock, irql);
}

void OrviaBridge::Render(const BYTE* data, ULONG bytes)
{
    KIRQL irql;
    KeAcquireSpinLock(&lock, &irql);
    const ULONGLONG now = KeQueryInterruptTime();
    if (now - lastRender > MaxAge) { count = 0; }
    lastRender = now;
    if (!protectedStream) {
        const ULONG keep = min(bytes, Capacity);
        data += bytes - keep;
        const ULONG first = min(keep, Capacity - head);
        RtlCopyMemory(samples + head, data, first);
        RtlCopyMemory(samples, data + first, keep - first);
        head = (head + keep) % Capacity;
        count = min(count + keep, Capacity);
    }
    KeReleaseSpinLock(&lock, irql);
}

void OrviaBridge::Capture(BYTE* data, ULONG bytes)
{
    KIRQL irql;
    KeAcquireSpinLock(&lock, &irql);
    if (KeQueryInterruptTime() - lastRender > MaxAge) { count = 0; }
    const ULONG available = min(bytes, count);
    const ULONG tail = (head + Capacity - count) % Capacity;
    const ULONG first = min(available, Capacity - tail);
    RtlCopyMemory(data, samples + tail, first);
    RtlCopyMemory(data + first, samples, available - first);
    count -= available;
    KeReleaseSpinLock(&lock, irql);
    RtlZeroMemory(data + available, bytes - available);
}
