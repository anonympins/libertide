use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

#[derive(Clone)]
pub struct AudioPlayer {
    sender: Sender<(u64, Vec<f32>, u32, Option<(i32, i32)>)>,
    ring_buffer: Arc<Mutex<VecDeque<f32>>>,
    generation: Arc<AtomicU64>,
}

/// Applique la spatialisation binaurale 3D (ITD + ILD + filtre d'élévation de pinna)
/// à un flux mono pour générer un flux stéréo entrelacé [L, R, L, R, ...]
pub fn spatialize_mono_to_stereo(
    mono_samples: &[f32],
    sample_rate: u32,
    click_x: i32,
    click_y: i32,
    screen_width: f32,
    screen_height: f32,
) -> Vec<f32> {
    if mono_samples.is_empty() {
        return Vec::new();
    }

    let sw = screen_width.max(640.0);
    let sh = screen_height.max(480.0);

    // Coordonnées normalisées : azimut x_norm dans [-1.0 (gauche), +1.0 (droite)]
    let x_norm = ((click_x as f32 / sw) * 2.0 - 1.0).clamp(-1.0, 1.0);
    // Élévation y_norm dans [-1.0 (bas), +1.0 (haut)]
    let y_norm = (1.0 - (click_y as f32 / sh) * 2.0).clamp(-1.0, 1.0);

    // 1. ILD (Interaural Level Difference) avec loi de panoramique à puissance constante (equal-power)
    let pan_angle = (x_norm + 1.0) * 0.25 * std::f32::consts::PI; // [0, PI/2]
    let mut gain_left = pan_angle.cos();
    let mut gain_right = pan_angle.sin();

    // 2. ITD (Interaural Time Difference) : retard physique entre les deux oreilles
    // Modèle de Woodworth : retard max de ~0.65 ms pour un azimut de 90°
    let max_delay_secs = 0.00065f32;
    let max_delay_samples = (max_delay_secs * sample_rate as f32).round() as usize;
    let delay_samples = ((x_norm.abs() * max_delay_samples as f32).round() as usize).min(max_delay_samples);

    // 3. Filtre spectral d'élévation (effet de pavillon de l'oreille / Pinna) :
    // Haut d'écran (y > 0) : accentuation de la clarté et des hautes fréquences (treble boost)
    // Bas d'écran (y < 0) : son légèrement étouffé/plus mat (treble cut)
    let elevation_brightness = 1.0 + (y_norm * 0.25);

    let out_len = (mono_samples.len() + max_delay_samples) * 2;
    let mut stereo = vec![0.0f32; out_len];

    // Filtre IIR 1er ordre pour l'ombre acoustique crânienne (Head Shadowing)
    let shadow_alpha = (0.28 + 0.42 * (1.0 - x_norm.abs())).clamp(0.15, 0.95);
    let mut left_filter_state = 0.0f32;
    let mut right_filter_state = 0.0f32;

    // Attribution des retards : si le son vient de droite, l'oreille gauche a du retard (et vice-versa)
    let (left_offset, right_offset) = if x_norm >= 0.0 {
        (delay_samples, 0)
    } else {
        (0, delay_samples)
    };

    for (i, &sample) in mono_samples.iter().enumerate() {
        let shaped = sample * elevation_brightness;

        // Oreille gauche
        let l_in = shaped * gain_left;
        let l_sample = if x_norm > 0.15 {
            // Ombre acoustique crânienne appliquée à l'oreille opposée
            left_filter_state += shadow_alpha * (l_in - left_filter_state);
            left_filter_state
        } else {
            l_in
        };
        let l_idx = (i + left_offset) * 2;
        if l_idx < stereo.len() {
            stereo[l_idx] = l_sample.clamp(-1.0, 1.0);
        }

        // Oreille droite
        let r_in = shaped * gain_right;
        let r_sample = if x_norm < -0.15 {
            right_filter_state += shadow_alpha * (r_in - right_filter_state);
            right_filter_state
        } else {
            r_in
        };
        let r_idx = (i + right_offset) * 2 + 1;
        if r_idx < stereo.len() {
            stereo[r_idx] = r_sample.clamp(-1.0, 1.0);
        }
    }

    stereo
}

impl AudioPlayer {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (tx, rx) = channel::<(u64, Vec<f32>, u32, Option<(i32, i32)>)>();
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
                        let mut buf = buf_cb.lock().unwrap_or_else(|e| e.into_inner());
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
                        let mut buf = buf_cb.lock().unwrap_or_else(|e| e.into_inner());
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

            loop {
                let recv_res = {
                    let rx_guard = rx.lock().unwrap_or_else(|e| e.into_inner());
                    rx_guard.recv()
                };
                let (gen, raw_samples, src_rate, spatial_coords) = match recv_res {
                    Ok(item) => item,
                    Err(_) => break,
                };
                if gen != generation_clone.load(Ordering::SeqCst) {
                    continue;
                }
                if raw_samples.is_empty() {
                    continue;
                }
                let converted = if let Some((cx, cy)) = spatial_coords {
                    // Spatialisation 3D binaurale de la source mono vers les canaux stéréo
                    #[cfg(windows)]
                    let (scr_w, scr_h) = {
                        let wa = crate::window_util::get_desktop_work_area();
                        ((wa.right - wa.left).max(800) as f32, (wa.bottom - wa.top).max(600) as f32)
                    };
                    #[cfg(not(windows))]
                    let (scr_w, scr_h) = (1920.0f32, 1080.0f32);

                    let stereo_samples = spatialize_mono_to_stereo(&raw_samples, src_rate, cx, cy, scr_w, scr_h);
                    if channels == 2 {
                        resample_interleaved(&stereo_samples, 2, src_rate, sample_rate, 2)
                    } else {
                        resample_audio(&raw_samples, src_rate, sample_rate, channels)
                    }
                } else {
                    resample_audio(&raw_samples, src_rate, sample_rate, channels)
                };
                if gen != generation_clone.load(Ordering::SeqCst) {
                    continue;
                }
                let mut buf = ring_buffer.lock().unwrap_or_else(|e| e.into_inner());
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
        let _ = self.sender.send((gen, samples, sample_rate, None));
    }

    /// Joue un flux audio mono en le spatialisant en 3D selon les coordonnées écran (X, Y)
    pub fn play_spatial_samples(&self, samples: Vec<f32>, sample_rate: u32, click_x: i32, click_y: i32) {
        let gen = self.generation.load(Ordering::SeqCst);
        let _ = self.sender.send((gen, samples, sample_rate, Some((click_x, click_y))));
    }

    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut buf = self.ring_buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.clear();
        crate::set_tts_audio_level(0.0);
    }

    pub fn is_playing(&self) -> bool {
        self.ring_buffer
            .lock()
            .map(|b| !b.is_empty())
            .unwrap_or_else(|e| !e.into_inner().is_empty())
    }
}

fn resample_interleaved(input: &[f32], in_channels: usize, src_rate: u32, dst_rate: u32, dst_channels: usize) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }
    if src_rate == dst_rate && in_channels == dst_channels {
        return input.to_vec();
    }
    let frames_in = input.len() / in_channels;
    let ratio = dst_rate as f64 / src_rate as f64;
    let frames_out = (frames_in as f64 * ratio) as usize;
    let mut out = Vec::with_capacity(frames_out * dst_channels);

    for f in 0..frames_out {
        let src_frame = f as f64 / ratio;
        let f_idx = (src_frame.floor() as usize).min(frames_in.saturating_sub(1));
        let frac = (src_frame - f_idx as f64) as f32;
        let next_f = (f_idx + 1).min(frames_in.saturating_sub(1));

        for ch in 0..dst_channels.min(in_channels) {
            let s0 = input[f_idx * in_channels + ch];
            let s1 = input[next_f * in_channels + ch];
            out.push(s0 + frac * (s1 - s0));
        }
    }
    out
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