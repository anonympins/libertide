use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Clone)]
pub struct AudioPlayer {
    sender: Sender<Vec<f32>>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (tx, rx) = channel::<Vec<f32>>();
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

            let sample_rate = default_config.sample_rate().0;
            let channels = default_config.channels() as usize;

            let ring_buffer: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
            let buf_cb = Arc::clone(&ring_buffer);

            let err_fn = |err| eprintln!("[Audio CPAL] Erreur de flux : {err}");

            let stream = match default_config.sample_format() {
                cpal::SampleFormat::F32 => device.build_output_stream(
                    &default_config.into(),
                    move |data: &mut [f32], _| {
                        let mut buf = buf_cb.lock().unwrap();
                        for sample in data.iter_mut() {
                            *sample = buf.pop_front().unwrap_or(0.0);
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

            while let Ok(raw_samples) = rx.lock().unwrap().recv() {
                if raw_samples.is_empty() {
                    continue;
                }
                let converted = resample_linear(&raw_samples, 24000, sample_rate, channels);
                let mut buf = ring_buffer.lock().unwrap();
                buf.extend(converted);
            }
        });

        Ok(Self { sender: tx })
    }

    pub fn play_samples(&self, samples: Vec<f32>) {
        let _ = self.sender.send(samples);
    }
}

fn resample_linear(input: &[f32], src_rate: u32, dst_rate: u32, dst_channels: usize) -> Vec<f32> {
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
    for i in 0..out_len {
        let src_idx = i as f64 / ratio;
        let idx0 = src_idx.floor() as usize;
        let idx1 = (idx0 + 1).min(input.len() - 1);
        let frac = (src_idx - idx0 as f64) as f32;
        let s = input[idx0] * (1.0 - frac) + input[idx1] * frac;
        for _ in 0..dst_channels {
            out.push(s);
        }
    }
    out
}