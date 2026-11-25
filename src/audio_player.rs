use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::thread;
use crate::fs_util::{read_env_var_from_file, write_env_var_to_file};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioPlaybackMode {
    MonoCentered,
    MonoSpatialized { click_x: i32, click_y: i32 },
    StereoInterleaved,
}

#[derive(Clone)]
pub struct AudioPlayer {
    sender: Sender<(u64, Vec<f32>, u32, AudioPlaybackMode)>,
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
        let (tx, rx) = channel::<(u64, Vec<f32>, u32, AudioPlaybackMode)>();
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
                let (gen, raw_samples, src_rate, playback_mode) = match recv_res {
                    Ok(item) => item,
                    Err(_) => break,
                };
                if gen != generation_clone.load(Ordering::SeqCst) {
                    continue;
                }
                if raw_samples.is_empty() {
                    continue;
                }
                let converted = match playback_mode {
                    AudioPlaybackMode::MonoSpatialized { click_x, click_y } => {
                        let (scr_w, scr_h) = crate::window_util::get_desktop_screen_dimensions();
                        let stereo_samples = spatialize_mono_to_stereo(&raw_samples, src_rate, click_x, click_y, scr_w, scr_h);
                        if channels == 2 {
                            resample_interleaved(&stereo_samples, 2, src_rate, sample_rate, 2)
                        } else {
                            resample_audio(&raw_samples, src_rate, sample_rate, channels)
                        }
                    }
                    AudioPlaybackMode::StereoInterleaved => {
                        if channels == 2 {
                            resample_interleaved(&raw_samples, 2, src_rate, sample_rate, 2)
                        } else {
                            let mut mono = Vec::with_capacity(raw_samples.len() / 2);
                            for chunk in raw_samples.chunks_exact(2) {
                                mono.push((chunk[0] + chunk[1]) * 0.5);
                            }
                            resample_audio(&mono, src_rate, sample_rate, channels)
                        }
                    }
                    AudioPlaybackMode::MonoCentered => {
                        resample_audio(&raw_samples, src_rate, sample_rate, channels)
                    }
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
        let _ = self.sender.send((gen, samples, sample_rate, AudioPlaybackMode::MonoCentered));
    }

    /// Joue un flux audio mono en le spatialisant en 3D selon les coordonnées écran (X, Y)
    pub fn play_spatial_samples(&self, samples: Vec<f32>, sample_rate: u32, click_x: i32, click_y: i32) {
        let gen = self.generation.load(Ordering::SeqCst);
        let _ = self.sender.send((gen, samples, sample_rate, AudioPlaybackMode::MonoSpatialized { click_x, click_y }));
    }

    /// Joue un flux audio stéréo entrelacé pré-spatialisé
    pub fn play_stereo_samples(&self, stereo_samples: Vec<f32>, sample_rate: u32) {
        let gen = self.generation.load(Ordering::SeqCst);
        let _ = self.sender.send((gen, stereo_samples, sample_rate, AudioPlaybackMode::StereoInterleaved));
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

static SHARED_PLAYER: Mutex<Option<AudioPlayer>> = Mutex::new(None);

pub fn get_or_init_audio_player() -> Option<AudioPlayer> {
    let mut guard = SHARED_PLAYER.lock().ok()?;
    if guard.is_none() {
        *guard = AudioPlayer::new().ok();
    }
    guard.clone()
}

// ============================================================================
// Carte Sonore Binaurale de l'Écran (Auditory Beacon Map)
// ============================================================================

pub static AUDITORY_BEACON_MAP_ENABLED: AtomicBool = AtomicBool::new(true);
static LAST_BEACON_SWEEP: Mutex<Option<Instant>> = Mutex::new(None);

pub fn is_auditory_beacon_map_enabled() -> bool {
    AUDITORY_BEACON_MAP_ENABLED.load(Ordering::Relaxed)
}

pub fn set_auditory_beacon_map_enabled(enabled: bool) {
    AUDITORY_BEACON_MAP_ENABLED.store(enabled, Ordering::Relaxed);
    let _ = write_env_var_to_file(".env", "AUDITORY_BEACON_MAP", if enabled { "true" } else { "false" });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeaconRole {
    Window,
    Button,
    Link,
    TextInput,
    TextReader,
    Heading,
    CheckboxOrToggle,
    Slider,
    Media,
    Other,
}

#[derive(Clone, Debug)]
pub struct AuditoryBeaconItem {
    pub click_x: i32,
    pub click_y: i32,
    pub role: BeaconRole,
    pub label: String,
}

impl From<&crate::types::NumberedElementTarget> for AuditoryBeaconItem {
    fn from(t: &crate::types::NumberedElementTarget) -> Self {
        let role = match t.role.as_str() {
            "bouton" | "button" => BeaconRole::Button,
            "lien" | "link" => BeaconRole::Link,
            "champ de saisie" | "input" | "zone de texte" | "textarea" => BeaconRole::TextInput,
            "lecture textuelle" => BeaconRole::TextReader,
            "onglet" | "tab" => BeaconRole::Button,
            "en-tête" | "heading" => BeaconRole::Heading,
            "case à cocher" | "checkbox" => BeaconRole::CheckboxOrToggle,
            "curseur" | "slider" => BeaconRole::Slider,
            "lecteur multimédia" | "media" => BeaconRole::Media,
            "fenêtre" | "window" => BeaconRole::Window,
            _ => {
                let lower = t.role.to_lowercase();
                if lower.contains("bouton") || lower.contains("button") {
                    BeaconRole::Button
                } else if lower.contains("lien") || lower.contains("link") {
                    BeaconRole::Link
                } else if t.is_editable() {
                    BeaconRole::TextInput
                } else {
                    BeaconRole::Other
                }
            }
        };
        Self {
            click_x: t.click_x,
            click_y: t.click_y,
            role,
            label: t.name.clone(),
        }
    }
}

impl From<&crate::automation::SwitchWindowTarget> for AuditoryBeaconItem {
    fn from(w: &crate::automation::SwitchWindowTarget) -> Self {
        Self {
            click_x: w.click_x,
            click_y: w.click_y,
            role: BeaconRole::Window,
            label: w.title.clone(),
        }
    }
}

const PENTATONIC_SCALE: &[f32] = &[
    196.00,  // G3
    220.00,  // A3
    261.63,  // C4
    293.66,  // D4
    329.63,  // E4
    392.00,  // G4
    440.00,  // A4
    523.25,  // C5
    587.33,  // D5
    659.25,  // E5
    783.99,  // G5
    880.00,  // A5
    1046.50, // C6
    1174.66, // D6
    1318.51, // E6
    1567.98, // G6
];

fn synth_beacon_tone(role: BeaconRole, freq: f32, sample_rate: u32) -> Vec<f32> {
    let sr = sample_rate as f32;
    let dur_secs = match role {
        BeaconRole::Window => 0.16,
        BeaconRole::Button => 0.09,
        BeaconRole::Link => 0.07,
        BeaconRole::TextInput => 0.11,
        BeaconRole::Heading => 0.13,
        _ => 0.08,
    };
    let total_samples = (dur_secs * sr) as usize;
    let mut out = Vec::with_capacity(total_samples);
    let tau = 2.0 * std::f32::consts::PI;

    for i in 0..total_samples {
        let t = i as f32 / sr;
        let progress = t / dur_secs;
        let env = (1.0 - progress).powf(1.8);

        let s = match role {
            BeaconRole::Window => {
                let f1 = (t * freq * tau).sin() * 0.65;
                let f0 = (t * (freq * 0.5) * tau).sin() * 0.35;
                (f1 + f0) * env * 0.70
            }
            BeaconRole::Button => {
                let f1 = (t * freq * tau).sin() * 0.70;
                let f2 = (t * freq * 2.0 * tau).sin() * 0.30;
                (f1 + f2) * env * 0.80
            }
            BeaconRole::Link => {
                let f1 = (t * freq * tau).sin() * 0.60;
                let f3 = (t * freq * 3.0 * tau).sin() * 0.40;
                (f1 + f3) * env * 0.65
            }
            BeaconRole::TextInput => {
                let cur_f = freq * (1.0 - progress * 0.15);
                let f1 = (t * cur_f * tau).sin();
                f1 * env * 0.75
            }
            BeaconRole::Heading => {
                let f1 = (t * freq * tau).sin() * 0.50;
                let f2 = (t * freq * 2.0 * tau).sin() * 0.30;
                let f4 = (t * freq * 4.0 * tau).sin() * 0.20;
                (f1 + f2 + f4) * env * 0.70
            }
            _ => {
                let f1 = (t * freq * tau).sin();
                f1 * env * 0.60
            }
        };
        out.push(s);
    }
    out
}

fn synth_breeze_sweep(duration_secs: f32, sample_rate: u32) -> Vec<f32> {
    let sr = sample_rate as f32;
    let total_samples = (duration_secs * sr) as usize;
    let mut stereo = vec![0.0f32; total_samples * 2];
    let pi = std::f32::consts::PI;

    let mut rng = 123456789u32;
    let mut smooth_noise = 0.0f32;

    for i in 0..total_samples {
        let t = i as f32 / total_samples as f32;
        let env = (t * pi).sin().powf(1.4) * 0.045;

        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let white = ((rng >> 16) as f32 / 32768.0) - 1.0;
        smooth_noise += 0.08 * (white - smooth_noise);

        let lfo = (t * 8.0 * pi).sin() * 0.25 + 0.75;
        let sample = smooth_noise * env * lfo;

        let pan_angle = t * 0.5 * pi;
        let gain_left = pan_angle.cos();
        let gain_right = pan_angle.sin();

        stereo[i * 2] = sample * gain_left;
        stereo[i * 2 + 1] = sample * gain_right;
    }
    stereo
}

pub fn generate_auditory_beacon_map(
    beacons: &[AuditoryBeaconItem],
    screen_width: f32,
    screen_height: f32,
    sample_rate: u32,
) -> Vec<f32> {
    if beacons.is_empty() {
        return Vec::new();
    }

    let mut filtered_beacons: Vec<&AuditoryBeaconItem> = beacons
        .iter()
        .filter(|b| b.click_x >= 0 && b.click_y >= 0)
        .collect();

    filtered_beacons.sort_by_key(|b| b.click_x);

    let mut clustered = Vec::new();
    for b in filtered_beacons {
        let too_close = clustered.iter().any(|c: &&AuditoryBeaconItem| {
            (c.click_x - b.click_x).abs() < 45 && (c.click_y - b.click_y).abs() < 35
        });
        if !too_close {
            clustered.push(b);
            if clustered.len() >= 16 {
                break;
            }
        }
    }

    let sweep_dur = 0.55f32;
    let total_dur = sweep_dur + 0.25f32;
    let total_samples = (total_dur * sample_rate as f32) as usize;
    let mut master_stereo = vec![0.0f32; total_samples * 2];

    let breeze = synth_breeze_sweep(sweep_dur, sample_rate);
    for (i, &s) in breeze.iter().enumerate() {
        if i < master_stereo.len() {
            master_stereo[i] += s;
        }
    }

    let beacon_gain = (0.50 / (clustered.len() as f32).sqrt().max(1.0)).clamp(0.12, 0.45);

    for beacon in clustered {
        let x_ratio = (beacon.click_x as f32 / screen_width.max(1.0)).clamp(0.0, 1.0);
        let y_ratio = (beacon.click_y as f32 / screen_height.max(1.0)).clamp(0.0, 1.0);

        let start_sample = (x_ratio * (sweep_dur - 0.10) * sample_rate as f32) as usize;
        let scale_idx = ((1.0 - y_ratio) * (PENTATONIC_SCALE.len() - 1) as f32).round() as usize;
        let freq = PENTATONIC_SCALE[scale_idx.min(PENTATONIC_SCALE.len() - 1)];

        let mono_tone = synth_beacon_tone(beacon.role, freq, sample_rate);
        let spatial_stereo = spatialize_mono_to_stereo(&mono_tone, sample_rate, beacon.click_x, beacon.click_y, screen_width, screen_height);

        let start_idx = start_sample * 2;
        for (i, &s) in spatial_stereo.iter().enumerate() {
            let dest = start_idx + i;
            if dest < master_stereo.len() {
                master_stereo[dest] += s * beacon_gain;
            }
        }
    }

    let max_peak = master_stereo.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    if max_peak > 0.85 {
        let scale = 0.85 / max_peak;
        for s in master_stereo.iter_mut() {
            *s *= scale;
        }
    }

    master_stereo
}

pub fn play_auditory_beacon_map_for_targets(targets: &[crate::types::NumberedElementTarget]) {
    if !is_auditory_beacon_map_enabled() || targets.is_empty() {
        return;
    }
    let now = Instant::now();
    if let Ok(mut lock) = LAST_BEACON_SWEEP.lock() {
        if let Some(last) = *lock {
            if now.duration_since(last) < Duration::from_millis(650) {
                return;
            }
        }
        *lock = Some(now);
    }

    let (scr_w, scr_h) = crate::window_util::get_desktop_screen_dimensions();
    let beacons: Vec<AuditoryBeaconItem> = targets.iter().map(AuditoryBeaconItem::from).collect();
    let sample_rate = 44100;
    let stereo = generate_auditory_beacon_map(&beacons, scr_w, scr_h, sample_rate);
    if let Some(player) = get_or_init_audio_player() {
        player.play_stereo_samples(stereo, sample_rate);
    }
}

pub fn play_auditory_beacon_map_for_windows(windows: &[crate::automation::SwitchWindowTarget]) {
    if !is_auditory_beacon_map_enabled() || windows.is_empty() {
        return;
    }
    let now = Instant::now();
    if let Ok(mut lock) = LAST_BEACON_SWEEP.lock() {
        if let Some(last) = *lock {
            if now.duration_since(last) < Duration::from_millis(650) {
                return;
            }
        }
        *lock = Some(now);
    }

    let (scr_w, scr_h) = crate::window_util::get_desktop_screen_dimensions();
    let beacons: Vec<AuditoryBeaconItem> = windows.iter().map(AuditoryBeaconItem::from).collect();
    let sample_rate = 44100;
    let stereo = generate_auditory_beacon_map(&beacons, scr_w, scr_h, sample_rate);
    if let Some(player) = get_or_init_audio_player() {
        player.play_stereo_samples(stereo, sample_rate);
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