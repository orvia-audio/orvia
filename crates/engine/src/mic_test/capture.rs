//! Preserve the negotiated sample width in the queue between capture and polling.
use super::{Consumer, Frame, Producer, RingBuffer};
use crate::formats::stream_config;
use anyhow::Result;
use cpal::{
    Device, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig, traits::DeviceTrait,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) enum CaptureQueue {
    Float(Consumer<Frame>),
    Mono(Consumer<f32>),
    Integer(Consumer<[i16; 2]>),
    MonoInteger(Consumer<i16>),
}

impl CaptureQueue {
    pub(super) fn open(
        device: &Device,
        format: &SupportedStreamConfig,
        capacity: usize,
        overflow: Arc<AtomicBool>,
        error: impl FnMut(cpal::Error) + Send + 'static,
    ) -> Result<(Stream, Self)> {
        let channels = usize::from(format.channels());
        let stereo = move |data: &[f32], tx: &mut Producer<Frame>| {
            if channels == 2 {
                tx.push_partial_slice(data.as_chunks::<2>().0).0.len()
            } else {
                push_channels(data, channels, tx)
            }
        };
        Ok(match (format.sample_format(), channels) {
            (SampleFormat::F32, 1) => {
                let (s, q) = open(device, format, capacity, overflow, error, |data, tx| {
                    tx.push_partial_slice(data).0.len()
                })?;
                (s, Self::Mono(q))
            }
            (SampleFormat::F32, _) => {
                let (s, q) = open(device, format, capacity, overflow, error, stereo)?;
                (s, Self::Float(q))
            }
            (SampleFormat::I16, 1) => {
                let (s, q) = open(device, format, capacity, overflow, error, |data, tx| {
                    tx.push_partial_slice(data).0.len()
                })?;
                (s, Self::MonoInteger(q))
            }
            (SampleFormat::I16, _) => {
                let (s, q) = open(
                    device,
                    format,
                    capacity,
                    overflow,
                    error,
                    move |data, tx| {
                        if channels == 2 {
                            tx.push_partial_slice(data.as_chunks::<2>().0).0.len()
                        } else {
                            push_channels(data, channels, tx)
                        }
                    },
                )?;
                (s, Self::Integer(q))
            }
            (sample, _) => anyhow::bail!("Unsupported recording format: {sample}"),
        })
    }

    pub(super) fn slots(&self) -> usize {
        match self {
            Self::Float(q) => q.slots(),
            Self::Mono(q) => q.slots(),
            Self::Integer(q) => q.slots(),
            Self::MonoInteger(q) => q.slots(),
        }
    }

    pub(super) fn drain(
        &mut self,
        count: usize,
        mut consume: impl FnMut(&[Frame]) -> Result<()>,
    ) -> Result<()> {
        if count == 0 {
            return Ok(());
        }
        match self {
            Self::Float(q) => read(q, count, consume),
            Self::Mono(q) => converted(q, count, |x| [x; 2], &mut consume),
            Self::Integer(q) => converted(q, count, |x| x.map(f32::from_sample), &mut consume),
            Self::MonoInteger(q) => converted(q, count, |x| [f32::from_sample(x); 2], &mut consume),
        }
    }
}

fn push_channels<T: Copy>(data: &[T], channels: usize, tx: &mut Producer<[T; 2]>) -> usize {
    let frames = data.chunks_exact(channels);
    let count = tx.slots().min(frames.len());
    tx.write_chunk_uninit(count).map_or(0, |chunk| {
        chunk.fill_from_iter(frames.map(|f| [f[0], f[1]]))
    })
}

fn open<T: SizedSample, U: Copy + Send + 'static>(
    device: &Device,
    format: &SupportedStreamConfig,
    capacity: usize,
    overflow: Arc<AtomicBool>,
    error: impl FnMut(cpal::Error) + Send + 'static,
    mut write: impl FnMut(&[T], &mut Producer<U>) -> usize + Send + 'static,
) -> Result<(Stream, Consumer<U>)> {
    let (mut tx, rx) = RingBuffer::new(capacity);
    let channels = usize::from(format.channels());
    let stream = device.build_input_stream(
        stream_config(format),
        move |data: &[T], _| {
            if write(data, &mut tx) < data.len() / channels {
                overflow.store(true, Ordering::Release);
            }
        },
        error,
        None,
    )?;
    Ok((stream, rx))
}

fn read<T>(
    q: &mut Consumer<T>,
    count: usize,
    mut consume: impl FnMut(&[T]) -> Result<()>,
) -> Result<()> {
    let chunk = q.read_chunk(count)?;
    let (first, second) = chunk.as_slices();
    consume(first)?;
    consume(second)?;
    chunk.commit_all();
    Ok(())
}

fn converted<T: Copy>(
    q: &mut Consumer<T>,
    count: usize,
    convert: impl Fn(T) -> Frame,
    consume: &mut impl FnMut(&[Frame]) -> Result<()>,
) -> Result<()> {
    let mut converted = [[0.; 2]; 1024];
    read(q, count, |samples| {
        for batch in samples.chunks(converted.len()) {
            for (&sample, dst) in batch.iter().zip(&mut converted) {
                *dst = convert(sample);
            }
            consume(&converted[..batch.len()])?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_capture_preserves_channel_duplication_signed_samples_and_wrap() {
        fn drain(mut queue: CaptureQueue) -> Vec<Frame> {
            let mut frames = Vec::new();
            queue
                .drain(queue.slots(), |chunk| {
                    frames.extend_from_slice(chunk);
                    Ok(())
                })
                .unwrap();
            frames
        }
        let ints = [-32768, -3, 0, 1, 32767];
        let (mut tx, mut rx) = RingBuffer::new(5);
        tx.push_entire_slice(&ints[..3]).unwrap();
        rx.pop().unwrap();
        rx.pop().unwrap();
        tx.push_entire_slice(&ints[..4]).unwrap();
        let expected = [0i16, -32768, -3, 0, 1].map(|s| [f32::from_sample(s); 2]);
        assert_eq!(drain(CaptureQueue::MonoInteger(rx)), expected);
        let (mut tx, rx) = RingBuffer::new(5);
        let stereo = ints.map(|s| [s, s.saturating_add(3)]);
        tx.push_entire_slice(&stereo).unwrap();
        assert_eq!(
            drain(CaptureQueue::Integer(rx)),
            stereo.map(|f| f.map(f32::from_sample))
        );
        let (mut tx, rx) = RingBuffer::new(5);
        tx.push_entire_slice(&[-0., -0.5, 0.125]).unwrap();
        assert_eq!(
            drain(CaptureQueue::Mono(rx))
                .into_iter()
                .map(|f| f.map(f32::to_bits))
                .collect::<Vec<_>>(),
            [-0.0_f32, -0.5, 0.125].map(|x| [x.to_bits(); 2])
        );
    }
}
