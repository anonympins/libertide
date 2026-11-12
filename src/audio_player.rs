use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

#[derive(Clone)]
pub struct AudioPlayer {
    sender: Sender<(u64, Vec<f32>, u32)>,
    ring_buffer: Arc<Mutex<VecDeque<f32>>>,
    generation: Arc<AtomicU64>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (tx, rx) = channel::<(u64, Vec<f32>, u32)>();
        let ring_buffer: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let ring_buffer_clone = Arc::clone(&ring_buffer);
        let generation = Arc::new(AtomicU64::new(0));
        let generation_clone = Arc::clone(&generation);
        let rx = Arc::new(Mutex::new(rx));

        thread::spawn(move || {
            let host = cpal::default_host();
            let device = match host.default_output_device() {
                Some(d) => d,
                None => {
                    eprintln!("[Audio] Aucun périphérique audio de sortie trouvé.");
                    return;
                }
            };

            let default_config = match device.default_output_config() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[Audio] Erreur lors de la récupération de la config audio : {e}");
                    return;
                }
            };

            let sample_rate = default_config.sample_rate();
            let channels = default_config.channels() as usize;

            let buf_cb = Arc::clone(&ring_buffer);

            let err_fn = |err| eprintln!("[Audio CPAL] Erreur de flux : {err}");

            let sample_format = default_config.sample_format();
            let stream_config: cpal::StreamConfig = default_config.into();
            let stream = match sample_format {
                cpal::SampleFormat::F32 => device.build_output_stream(
                    stream_config,
                    move |data: &mut [f32], _| {
                        let mut buf = buf_cb.lock().unwrap();
                        let mut sum_sq = 0.0f32;
                        for sample in data.iter_mut() {
                            let s = buf.pop_front().unwrap_or(0.0);
                            *sample = s;
                            sum_sq += s * s;
                        }
                        if !data.is_empty() {
                            let rms = (sum_sq / data.len() as f32).sqrt();
                            let level = (rms * 4.5).clamp(0.0, 1.0);
                            crate::set_tts_audio_level(level);
                        }
                    },
                    err_fn,
                    None,
                ),
                cpal::SampleFormat::I16 => device.build_output_stream(
                    stream_config,
                    move |data: &mut [i16], _| {
                        let mut buf = buf_cb.lock().unwrap();
                        let mut sum_sq = 0.0f32;
                        for sample in data.iter_mut() {
                            let s = buf.pop_front().unwrap_or(0.0);
                            *sample = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                            sum_sq += s * s;
                        }
                        if !data.is_empty() {
                            let rms = (sum_sq / data.len() as f32).sqrt();
                            let level = (rms * 4.5).clamp(0.0, 1.0);
                            crate::set_tts_audio_level(level);
                        }
                    },
                    err_fn,
                    None,
                ),
                _ => {
                    eprintln!("[Audio] Format d'échantillonnage de la carte son non pris en charge.");
                    return;
                }
            };

            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[Audio] Impossible d'initialiser le flux de sortie : {e}");
                    return;
                }
            };

            if let Err(e) = stream.play() {
                eprintln!("[Audio] Erreur lors du démarrage de la lecture : {e}");
                return;
            }

            while let Ok((gen, raw_samples, src_rate)) = rx.lock().unwrap().recv() {
                if gen != generation_clone.load(Ordering::SeqCst) {
                    continue;
                }
                if raw_samples.is_empty() {
                    continue;
                }
                let converted = resample_audio(&raw_samples, src_rate, sample_rate, channels);
                if gen != generation_clone.load(Ordering::SeqCst) {
                    continue;
                }
                let mut buf = ring_buffer.lock().unwrap();
                buf.extend(converted);
            }
        });

        Ok(Self {
            sender: tx,
            ring_buffer: ring_buffer_clone,
            generation,
        })
    }

    pub fn play_samples(&self, samples: Vec<f32>, sample_rate: u32) {
        let gen = self.generation.load(Ordering::SeqCst);
        let _ = self.sender.send((gen, samples, sample_rate));
    }

    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut buf) = self.ring_buffer.lock() {
            buf.clear();
        }
        crate::set_tts_audio_level(0.0);
    }

    pub fn is_playing(&self) -> bool {
        self.ring_buffer.lock().map(|b| !b.is_empty()).unwrap_or(false)
    }
}

fn resample_audio(input: &[f32], src_rate: u32, dst_rate: u32, dst_channels: usize) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }
    if src_rate == dst_rate {
        if dst_channels == 1 {
            return input.to_vec();
        }
        let mut out = Vec::with_capacity(input.len() * dst_channels);
        for &s in input {
            for _ in 0..dst_channels {
                out.push(s);
            }
        }
        return out;
    }
    let ratio = dst_rate as f64 / src_rate as f64;
    let out_len = (input.len() as f64 * ratio) as usize;
    let mut out = Vec::with_capacity(out_len * dst_channels);
    let max_idx = input.len().saturating_sub(1);

    for i in 0..out_len {
        let src_idx = i as f64 / ratio;
        let idx1 = src_idx.floor() as usize;
        let idx0 = idx1.saturating_sub(1);
        let idx2 = (idx1 + 1).min(max_idx);
        let idx3 = (idx1 + 2).min(max_idx);

        let t = (src_idx - idx1 as f64) as f32;
        let t2 = t * t;
        let t3 = t2 * t;

        let p0 = input[idx0];
        let p1 = input[idx1.min(max_idx)];
        let p2 = input[idx2];
        let p3 = input[idx3];

        let s = 0.5 * (
            (2.0 * p1) +
            (-p0 + p2) * t +
            (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 +
            (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3
        );

        for _ in 0..dst_channels {
            out.push(s);
        }
    }
    out
}