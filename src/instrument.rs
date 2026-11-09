use std::f32::consts::PI;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use eframe::egui;
use rust_i18n::t;
use crate::overlay::{apply_dwell_to_response, DwellState};
use crate::theme::ThemeColors;

fn sound_bank_label(bank: SoundBank) -> String {
    match bank {
        SoundBank::ElectroBeat => t!("instrument.bank_electro").to_string(),
        SoundBank::ZenMeditation => t!("instrument.bank_zen").to_string(),
        SoundBank::RetroArcade => t!("instrument.bank_arcade").to_string(),
    }
}

fn localized_pad_labels(bank: SoundBank, lang: &str) -> [(&'static str, &'static str, &'static str); 8] {
    let is_azerty = lang == "fr";
    let k1 = if is_azerty { "[1] / [A]" } else { "[1] / [Q]" };
    let k2 = if is_azerty { "[2] / [Z]" } else { "[2] / [W]" };

    match bank {
        SoundBank::ElectroBeat => match lang {
            "es" => [
                ("🥁 Bombo", "Do / C2", k1),
                ("💥 Caja", "Re / D2", k2),
                ("🎩 Charles", "Mi / E2", "[3] / [E]"),
                ("👏 Palmas", "Sol / G2", "[4] / [R]"),
                ("🎸 Bajo", "La / A2", "[5] / [T]"),
                ("🎹 Sintetizador 1", "Do / C4", "[6] / [Y]"),
                ("✨ Sintetizador 2", "Sol / G4", "[7] / [U]"),
                ("⚡ Láser FX", "Cósmico", "[8] / [I]"),
            ],
            "de" => [
                ("🥁 Kick", "C2", k1),
                ("💥 Snare", "D2", k2),
                ("🎩 Hi-Hat", "E2", "[3] / [E]"),
                ("👏 Clap", "G2", "[4] / [R]"),
                ("🎸 Bass", "A2", "[5] / [T]"),
                ("🎹 Synth 1", "C4", "[6] / [Y]"),
                ("✨ Synth 2", "G4", "[7] / [U]"),
                ("⚡ Laser FX", "Kosmisch", "[8] / [I]"),
            ],
            "fr" => [
                ("🥁 Kick", "Do / C2", k1),
                ("💥 Snare", "Ré / D2", k2),
                ("🎩 Hi-Hat", "Mi / E2", "[3] / [E]"),
                ("👏 Clap", "Sol / G2", "[4] / [R]"),
                ("🎸 Basse", "La / A2", "[5] / [T]"),
                ("🎹 Synthé 1", "Do / C4", "[6] / [Y]"),
                ("✨ Synthé 2", "Sol / G4", "[7] / [U]"),
                ("⚡ Laser FX", "Cosmique", "[8] / [I]"),
            ],
            _ => [
                ("🥁 Kick", "C2", k1),
                ("💥 Snare", "D2", k2),
                ("🎩 Hi-Hat", "E2", "[3] / [E]"),
                ("👏 Clap", "G2", "[4] / [R]"),
                ("🎸 Bass", "A2", "[5] / [T]"),
                ("🎹 Synth 1", "C4", "[6] / [Y]"),
                ("✨ Synth 2", "G4", "[7] / [U]"),
                ("⚡ Laser FX", "Cosmic", "[8] / [I]"),
            ],
        },
        SoundBank::ZenMeditation => match lang {
            "es" => [
                ("🔔 Campanilla 1", "Do / C4", k1),
                ("🔔 Campanilla 2", "Re / D4", k2),
                ("🔔 Campanilla 3", "Mi / E4", "[3] / [E]"),
                ("🔔 Campanilla 4", "Sol / G4", "[4] / [R]"),
                ("🔔 Campanilla 5", "La / A4", "[5] / [T]"),
                ("🔔 Campanilla 6", "Do / C5", "[6] / [Y]"),
                ("🧘 Cuenco Tibetano", "Gong", "[7] / [U]"),
                ("✨ Estrella Zen", "Eco", "[8] / [I]"),
            ],
            "de" => [
                ("🔔 Klangspiel 1", "C4", k1),
                ("🔔 Klangspiel 2", "D4", k2),
                ("🔔 Klangspiel 3", "E4", "[3] / [E]"),
                ("🔔 Klangspiel 4", "G4", "[4] / [R]"),
                ("🔔 Klangspiel 5", "A4", "[5] / [T]"),
                ("🔔 Klangspiel 6", "C5", "[6] / [Y]"),
                ("🧘 Klangschale", "Gong", "[7] / [U]"),
                ("✨ Zen-Stern", "Echo", "[8] / [I]"),
            ],
            "fr" => [
                ("🔔 Carillon 1", "Do / C4", k1),
                ("🔔 Carillon 2", "Ré / D4", k2),
                ("🔔 Carillon 3", "Mi / E4", "[3] / [E]"),
                ("🔔 Carillon 4", "Sol / G4", "[4] / [R]"),
                ("🔔 Carillon 5", "La / A4", "[5] / [T]"),
                ("🔔 Carillon 6", "Do / C5", "[6] / [Y]"),
                ("🧘 Bol Tibétain", "Gong", "[7] / [U]"),
                ("✨ Étoile Zen", "Écho", "[8] / [I]"),
            ],
            _ => [
                ("🔔 Chime 1", "C4", k1),
                ("🔔 Chime 2", "D4", k2),
                ("🔔 Chime 3", "E4", "[3] / [E]"),
                ("🔔 Chime 4", "G4", "[4] / [R]"),
                ("🔔 Chime 5", "A4", "[5] / [T]"),
                ("🔔 Chime 6", "C5", "[6] / [Y]"),
                ("🧘 Tibetan Bowl", "Gong", "[7] / [U]"),
                ("✨ Zen Star", "Echo", "[8] / [I]"),
            ],
        },
        SoundBank::RetroArcade => match lang {
            "fr" => [
                ("👾 Saut", "8-Bit", k1),
                ("🪙 Pièce", "Double", k2),
                ("⚡ Laser", "Balayage", "[3] / [E]"),
                ("💥 Impact", "Bruit", "[4] / [R]"),
                ("🕹️ Bonus", "Arpège", "[5] / [T]"),
                ("🔊 Basse", "Carrée", "[6] / [Y]"),
                ("🌟 Zap", "Impulsion", "[7] / [U]"),
                ("🏆 Victoire", "Accord", "[8] / [I]"),
            ],
            _ => [
                ("👾 Jump", "8-Bit", k1),
                ("🪙 Coin", "Dual tone", k2),
                ("⚡ Laser", "Sweep", "[3] / [E]"),
                ("💥 Hit", "Noise", "[4] / [R]"),
                ("🕹️ Power-Up", "Arp", "[5] / [T]"),
                ("🔊 Square Bass", "Lead", "[6] / [Y]"),
                ("🌟 Zap", "Pulse", "[7] / [U]"),
                ("🏆 Victory", "Chord", "[8] / [I]"),
            ],
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SoundBank {
    ElectroBeat,
    ZenMeditation,
    RetroArcade,
}

struct ActiveVoice {
    samples: Vec<f32>,
    cursor: usize,
}

static ACTIVE_VOICES: Mutex<Vec<ActiveVoice>> = Mutex::new(Vec::new());
static CPAL_STREAM: Mutex<Option<cpal::Stream>> = Mutex::new(None);
static OUTPUT_SAMPLE_RATE: AtomicU32 = AtomicU32::new(44100);

pub fn init_instrument_audio() {
    let mut stream_guard = match CPAL_STREAM.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if stream_guard.is_some() {
        return;
    }

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        return;
    };
    let Ok(config) = device.default_output_config() else {
        return;
    };

    let sample_rate = config.sample_rate();
    let channels = config.channels() as usize;
    OUTPUT_SAMPLE_RATE.store(sample_rate, Ordering::Relaxed);

    let stream_config: cpal::StreamConfig = config.into();
    let err_fn = |err| eprintln!("[Instrument Audio] Erreur: {err}");

    let stream_res = match stream_config.clone() {
        _ => match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                stream_config,
                move |data: &mut [f32], _| {
                    let mut voices = match ACTIVE_VOICES.lock() {
                        Ok(v) => v,
                        Err(_) => return,
                    };
                    for frame in data.chunks_mut(channels) {
                        let mut mixed = 0.0f32;
                        for voice in voices.iter_mut() {
                            if voice.cursor < voice.samples.len() {
                                mixed += voice.samples[voice.cursor];
                                voice.cursor += 1;
                            }
                        }
                        let s = mixed.clamp(-1.0, 1.0);
                        for sample in frame.iter_mut() {
                            *sample = s;
                        }
                    }
                    voices.retain(|v| v.cursor < v.samples.len());
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                stream_config,
                move |data: &mut [i16], _| {
                    let mut voices = match ACTIVE_VOICES.lock() {
                        Ok(v) => v,
                        Err(_) => return,
                    };
                    for frame in data.chunks_mut(channels) {
                        let mut mixed = 0.0f32;
                        for voice in voices.iter_mut() {
                            if voice.cursor < voice.samples.len() {
                                mixed += voice.samples[voice.cursor];
                                voice.cursor += 1;
                            }
                        }
                        let s = (mixed.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        for sample in frame.iter_mut() {
                            *sample = s;
                        }
                    }
                    voices.retain(|v| v.cursor < v.samples.len());
                },
                err_fn,
                None,
            ),
            _ => return,
        },
    };

    if let Ok(s) = stream_res {
        if s.play().is_ok() {
            *stream_guard = Some(s);
        }
    }
}

pub fn play_samples(samples: Vec<f32>) {
    init_instrument_audio();
    if let Ok(mut voices) = ACTIVE_VOICES.lock() {
        if voices.len() > 14 {
            let drop_count = voices.len() - 14;
            voices.drain(0..drop_count);
        }
        voices.push(ActiveVoice { samples, cursor: 0 });
    }
}

// --- Synthèse sonore procédurale pure en temps réel ---

pub fn synth_kick(pitch_mult: f32, decay_mult: f32) -> Vec<f32> {
    let sr = OUTPUT_SAMPLE_RATE.load(Ordering::Relaxed).max(22050) as f32;
    let dur_secs = (0.24 * decay_mult).clamp(0.12, 0.6);
    let total = (sr * dur_secs) as usize;
    let mut samples = Vec::with_capacity(total);
    let start_freq = 160.0 * pitch_mult;
    let end_freq = 42.0 * pitch_mult;
    let mut phase = 0.0f32;

    for i in 0..total {
        let t = i as f32 / sr;
        let progress = t / dur_secs;
        let freq = end_freq + (start_freq - end_freq) * (1.0 - progress).powf(3.2);
        phase += 2.0 * PI * freq / sr;
        if phase > 2.0 * PI { phase -= 2.0 * PI; }
        let env = (-t * 14.0 / decay_mult.max(0.2)).exp();
        let click = if t < 0.005 { (1.0 - t / 0.005) * 0.4 } else { 0.0 };
        samples.push(((phase.sin() * env + click) * 0.85).clamp(-1.0, 1.0));
    }
    samples
}

pub fn synth_snare(pitch_mult: f32, snappy: f32) -> Vec<f32> {
    let sr = OUTPUT_SAMPLE_RATE.load(Ordering::Relaxed).max(22050) as f32;
    let dur_secs = 0.20;
    let total = (sr * dur_secs) as usize;
    let mut samples = Vec::with_capacity(total);
    let tone_freq = 185.0 * pitch_mult;
    let mut phase = 0.0f32;
    let mut rng = 123456789u32;

    for i in 0..total {
        let t = i as f32 / sr;
        phase += 2.0 * PI * tone_freq / sr;
        let tone = phase.sin() * (-t * 20.0).exp() * 0.45;
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let noise = ((rng >> 16) as f32 / 32768.0) - 1.0;
        let noise_env = (-t * (13.0 / snappy.max(0.3))).exp();
        samples.push(((tone + noise * noise_env * 0.5 * snappy) * 0.8).clamp(-1.0, 1.0));
    }
    samples
}

pub fn synth_hihat(open: bool, brightness: f32) -> Vec<f32> {
    let sr = OUTPUT_SAMPLE_RATE.load(Ordering::Relaxed).max(22050) as f32;
    let dur_secs = if open { 0.28 } else { 0.06 };
    let total = (sr * dur_secs) as usize;
    let mut samples = Vec::with_capacity(total);
    let mut rng = 987654321u32;
    let mut prev = 0.0f32;

    for i in 0..total {
        let t = i as f32 / sr;
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let raw = ((rng >> 16) as f32 / 32768.0) - 1.0;
        let hp = raw - prev * 0.85;
        prev = raw;
        let decay = if open { 13.0 } else { 45.0 };
        let env = (-t * decay).exp();
        samples.push((hp * env * brightness * 0.55).clamp(-1.0, 1.0));
    }
    samples
}

pub fn synth_clap(reverb: f32) -> Vec<f32> {
    let sr = OUTPUT_SAMPLE_RATE.load(Ordering::Relaxed).max(22050) as f32;
    let dur_secs = 0.24;
    let total = (sr * dur_secs) as usize;
    let mut samples = Vec::with_capacity(total);
    let mut rng = 555666777u32;

    for i in 0..total {
        let t = i as f32 / sr;
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let noise = ((rng >> 16) as f32 / 32768.0) - 1.0;
        let env = if t < 0.012 {
            (-t * 80.0).exp()
        } else if t < 0.024 {
            (-(t - 0.012) * 80.0).exp()
        } else if t < 0.036 {
            (-(t - 0.024) * 80.0).exp()
        } else {
            (-(t - 0.036) * (14.0 / reverb.max(0.2))).exp()
        };
        samples.push((noise * env * 0.75).clamp(-1.0, 1.0));
    }
    samples
}

pub fn synth_note(freq_hz: f32, dur_secs: f32, bend_ratio: f32, cutoff_mod: f32, is_bass: bool) -> Vec<f32> {
    let sr = OUTPUT_SAMPLE_RATE.load(Ordering::Relaxed).max(22050) as f32;
    let dur = dur_secs.clamp(0.18, 1.4);
    let total = (sr * dur) as usize;
    let mut samples = Vec::with_capacity(total);
    let mut phase = 0.0f32;
    let mut lfo = 0.0f32;

    for i in 0..total {
        let t = i as f32 / sr;
        let progress = t / dur;
        let cur_freq = freq_hz * (1.0 + (bend_ratio - 1.0) * (1.0 - (-progress * 4.0).exp()));
        lfo += 2.0 * PI * 5.0 / sr;
        let vibrato = 1.0 + 0.012 * lfo.sin() * progress;
        phase += 2.0 * PI * (cur_freq * vibrato) / sr;
        if phase > 2.0 * PI { phase -= 2.0 * PI; }

        let raw = if is_bass {
            phase.sin() * 0.72 + (2.0 * phase).sin() * 0.22 + (3.0 * phase).sin() * 0.06
        } else {
            let h1 = phase.sin();
            let h2 = (2.0 * phase).sin() * (0.35 + 0.3 * cutoff_mod);
            let h3 = (3.0 * phase).sin() * (0.20 + 0.25 * cutoff_mod);
            (h1 + h2 + h3) * 0.6
        };

        let env = if t < 0.015 { t / 0.015 } else { (-progress * 2.2).exp() };
        samples.push((raw * env * 0.75).clamp(-1.0, 1.0));
    }
    samples
}

pub fn synth_zen_bell(freq_hz: f32, shimmer: f32) -> Vec<f32> {
    let sr = OUTPUT_SAMPLE_RATE.load(Ordering::Relaxed).max(22050) as f32;
    let dur = 1.4;
    let total = (sr * dur) as usize;
    let mut samples = Vec::with_capacity(total);
    let mut p1 = 0.0f32;
    let mut p2 = 0.0f32;

    for i in 0..total {
        let t = i as f32 / sr;
        p1 += 2.0 * PI * freq_hz / sr;
        p2 += 2.0 * PI * (freq_hz * 2.756) / sr;
        let env1 = (-t * 1.8).exp();
        let env2 = (-t * 2.8).exp() * shimmer;

        let tone = (p1.sin() * env1 * 0.68 + p2.sin() * env2 * 0.32) * 0.8;
        samples.push(tone.clamp(-1.0, 1.0));
    }
    samples
}

pub fn synth_arcade(kind: usize) -> Vec<f32> {
    let sr = OUTPUT_SAMPLE_RATE.load(Ordering::Relaxed).max(22050) as f32;
    let dur = match kind {
        0 => 0.18,
        1 => 0.22,
        2 => 0.24,
        _ => 0.15,
    };
    let total = (sr * dur) as usize;
    let mut samples = Vec::with_capacity(total);
    let mut phase = 0.0f32;

    for i in 0..total {
        let t = i as f32 / sr;
        let progress = t / dur;
        let freq = match kind {
            0 => 160.0 + progress * 480.0,
            1 => if progress < 0.45 { 988.0 } else { 1318.0 },
            2 => 1400.0 * (1.0 - progress).powf(2.0) + 120.0,
            _ => 220.0 * (1.0 - progress),
        };
        phase += 2.0 * PI * freq / sr;
        if phase > 2.0 * PI { phase -= 2.0 * PI; }
        let sq = if phase.sin() > 0.0 { 0.38 } else { -0.38 };
        let env = (1.0 - progress).powf(1.1);
        samples.push(sq * env);
    }
    samples
}

#[derive(Clone)]
pub struct InstrumentState {
    pub bank: SoundBank,
    pub bpm: u32,
    pub single_switch_mode: bool,
    pub auto_scan: bool,
    pub active_pad_index: usize,
    pub last_scan_tick: Instant,
    pub auto_beat: bool,
    pub last_beat_tick: Instant,
    pub beat_step: usize,
    pub wah_cutoff: f32,
    pub pitch_bend: f32,
    pub echo_enabled: bool,
    pub press_start: Option<Instant>,
    pub pad_glow: [f32; 8],
}

impl Default for InstrumentState {
    fn default() -> Self {
        Self {
            bank: SoundBank::ElectroBeat,
            bpm: 105,
            single_switch_mode: true,
            auto_scan: true,
            active_pad_index: 0,
            last_scan_tick: Instant::now(),
            auto_beat: false,
            last_beat_tick: Instant::now(),
            beat_step: 0,
            wah_cutoff: 0.5,
            pitch_bend: 0.0,
            echo_enabled: true,
            press_start: None,
            pad_glow: [0.0; 8],
        }
    }
}

pub static INSTRUMENT_STATE: Mutex<Option<InstrumentState>> = Mutex::new(None);

impl InstrumentState {
    pub fn trigger_pad(&mut self, pad_idx: usize, hold_secs: f32) {
        let pad = pad_idx % 8;
        self.pad_glow[pad] = 1.0;

        let hold_bend = (hold_secs * 4.0).clamp(0.0, 7.0);
        let total_bend_semitones = self.pitch_bend + hold_bend;
        let pitch_mult = (2.0f32).powf(total_bend_semitones / 12.0);
        let cutoff = (self.wah_cutoff + hold_secs * 0.8).clamp(0.0, 1.0);

        let mut samples = match self.bank {
            SoundBank::ElectroBeat => match pad {
                0 => synth_kick(pitch_mult, 1.0 + hold_secs),
                1 => synth_snare(pitch_mult, cutoff),
                2 => synth_hihat(hold_secs > 0.15, cutoff),
                3 => synth_clap(1.0 + hold_secs * 2.0),
                4 => synth_note(65.41 * pitch_mult, 0.4 + hold_secs * 0.8, pitch_mult, cutoff, true),
                5 => synth_note(261.63 * pitch_mult, 0.35 + hold_secs * 0.6, pitch_mult, cutoff, false),
                6 => synth_note(392.00 * pitch_mult, 0.35 + hold_secs * 0.6, pitch_mult, cutoff, false),
                _ => synth_arcade(2),
            },
            SoundBank::ZenMeditation => {
                let pentatonic_freqs = [261.63, 293.66, 329.63, 392.00, 440.00, 523.25, 130.81, 659.25];
                if pad == 6 {
                    synth_note(130.81 * pitch_mult, 1.2 + hold_secs, pitch_mult, cutoff, true)
                } else {
                    synth_zen_bell(pentatonic_freqs[pad] * pitch_mult, cutoff)
                }
            }
            SoundBank::RetroArcade => synth_arcade(pad % 3),
        };

        if self.echo_enabled {
            let sr = OUTPUT_SAMPLE_RATE.load(Ordering::Relaxed).max(22050) as f32;
            let delay_samples = (sr * 0.22) as usize;
            let orig_len = samples.len();
            samples.resize(orig_len + delay_samples * 2, 0.0);
            for i in 0..orig_len {
                if i + delay_samples < samples.len() {
                    samples[i + delay_samples] += samples[i] * 0.42;
                }
                if i + delay_samples * 2 < samples.len() {
                    samples[i + delay_samples * 2] += samples[i] * 0.18;
                }
            }
        }

        play_samples(samples);
    }

    pub fn tick(&mut self) {
        let now = Instant::now();

        for glow in self.pad_glow.iter_mut() {
            *glow = (*glow * 0.82).max(0.0);
        }

        if self.single_switch_mode && self.auto_scan {
            let scan_interval = Duration::from_millis(((60.0 / self.bpm as f32) * 1000.0) as u64);
            if now.duration_since(self.last_scan_tick) >= scan_interval {
                self.last_scan_tick = now;
                self.active_pad_index = (self.active_pad_index + 1) % 8;
            }
        }

        if self.auto_beat {
            let beat_interval = Duration::from_millis(((60.0 / self.bpm as f32) * 500.0) as u64);
            if now.duration_since(self.last_beat_tick) >= beat_interval {
                self.last_beat_tick = now;
                self.beat_step = (self.beat_step + 1) % 4;
                match self.beat_step {
                    0 => play_samples(synth_kick(1.0, 0.8)),
                    1 => play_samples(synth_hihat(false, 0.6)),
                    2 => play_samples(synth_snare(1.0, 0.8)),
                    _ => play_samples(synth_hihat(false, 0.6)),
                }
            }
        }
    }
}

pub fn render_instrument_view(ui: &mut egui::Ui, dwell_state: &mut DwellState, theme: &ThemeColors) {
    let mut state = {
        let mut guard = INSTRUMENT_STATE.lock().unwrap();
        if guard.is_none() {
            *guard = Some(InstrumentState::default());
        }
        guard.as_ref().cloned().unwrap()
    };

    let loc = rust_i18n::locale();
    let lang = loc.split(['-', '_']).next().unwrap_or("fr").to_lowercase();
    state.tick();

    let pad_colors = [
        egui::Color32::from_rgb(255, 51, 102),
        egui::Color32::from_rgb(255, 153, 0),
        egui::Color32::from_rgb(255, 214, 0),
        egui::Color32::from_rgb(0, 230, 118),
        egui::Color32::from_rgb(0, 229, 255),
        egui::Color32::from_rgb(41, 121, 255),
        egui::Color32::from_rgb(124, 77, 255),
        egui::Color32::from_rgb(224, 64, 251),
    ];

    let pad_labels = localized_pad_labels(state.bank, &lang);
    let mut single_switch_hit = false;

    let mut triggered_pad = None;
    ui.input(|i| {
        if i.key_pressed(egui::Key::Space) || i.key_pressed(egui::Key::Enter) {
            single_switch_hit = true;
        }
        let num_keys = [
            egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4,
            egui::Key::Num5, egui::Key::Num6, egui::Key::Num7, egui::Key::Num8,
        ];
        for (idx, &k) in num_keys.iter().enumerate() {
            if i.key_pressed(k) {
                triggered_pad = Some(idx);
            }
        }
        // Support universel QWERTY & AZERTY
        if i.key_pressed(egui::Key::A) || i.key_pressed(egui::Key::Q) {
            triggered_pad = Some(0);
        } else if i.key_pressed(egui::Key::Z) || i.key_pressed(egui::Key::W) {
            triggered_pad = Some(1);
        } else if i.key_pressed(egui::Key::E) { triggered_pad = Some(2); }
        else if i.key_pressed(egui::Key::R) { triggered_pad = Some(3); }
        else if i.key_pressed(egui::Key::T) { triggered_pad = Some(4); }
        else if i.key_pressed(egui::Key::Y) { triggered_pad = Some(5); }
        else if i.key_pressed(egui::Key::U) { triggered_pad = Some(6); }
        else if i.key_pressed(egui::Key::I) { triggered_pad = Some(7); }
    });

    if single_switch_hit {
        triggered_pad = Some(state.active_pad_index);
    }

    if let Some(pad) = triggered_pad {
        state.trigger_pad(pad, 0.0);
    }

    // 1. Barre supérieure
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(t!("instrument.sound_bank")).strong().size(11.5).color(theme.accent));
        let b1 = ui.selectable_label(state.bank == SoundBank::ElectroBeat, sound_bank_label(SoundBank::ElectroBeat));
        if b1.clicked() { state.bank = SoundBank::ElectroBeat; }
        let b2 = ui.selectable_label(state.bank == SoundBank::ZenMeditation, sound_bank_label(SoundBank::ZenMeditation));
        if b2.clicked() { state.bank = SoundBank::ZenMeditation; }
        let b3 = ui.selectable_label(state.bank == SoundBank::RetroArcade, sound_bank_label(SoundBank::RetroArcade));
        if b3.clicked() { state.bank = SoundBank::RetroArcade; }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.checkbox(&mut state.echo_enabled, t!("instrument.echo"));
            ui.checkbox(&mut state.auto_beat, t!("instrument.beat_looper"));
        });
    });

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    // 2. Grille des 8 Pads
    let total_w = ui.available_width();
    let pad_w = ((total_w - 3.0 * 8.0) / 4.0).max(65.0);
    let pad_h = 75.0;

    for row in 0..2 {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for col in 0..4 {
                let idx = row * 4 + col;
                let is_scanned = state.single_switch_mode && state.active_pad_index == idx;
                let glow = state.pad_glow[idx];
                let base_color = pad_colors[idx];
                let (title, subtitle, shortcut) = pad_labels[idx];

                let fill_color = if glow > 0.05 {
                    egui::Color32::from_rgb(
                        (base_color.r() as f32 * (1.0 + glow * 0.3)).min(255.0) as u8,
                        (base_color.g() as f32 * (1.0 + glow * 0.3)).min(255.0) as u8,
                        (base_color.b() as f32 * (1.0 + glow * 0.3)).min(255.0) as u8,
                    )
                } else {
                    egui::Color32::from_rgba_unmultiplied(base_color.r(), base_color.g(), base_color.b(), 185)
                };

                let stroke = if is_scanned {
                    egui::Stroke::new(3.5, egui::Color32::WHITE)
                } else {
                    egui::Stroke::new(1.2, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 80))
                };

                let (rect, resp) = ui.allocate_exact_size(egui::vec2(pad_w, pad_h), egui::Sense::click());
                let p = ui.painter();
                p.rect(rect, egui::CornerRadius::same(10), fill_color, stroke, egui::StrokeKind::Inside);

                p.text(egui::pos2(rect.center().x, rect.min.y + 18.0), egui::Align2::CENTER_CENTER, title, egui::FontId::proportional(12.5), egui::Color32::WHITE);
                p.text(egui::pos2(rect.center().x, rect.min.y + 38.0), egui::Align2::CENTER_CENTER, subtitle, egui::FontId::proportional(11.0), egui::Color32::from_rgba_unmultiplied(255, 255, 255, 220));
                p.text(egui::pos2(rect.center().x, rect.max.y - 12.0), egui::Align2::CENTER_CENTER, shortcut, egui::FontId::proportional(10.0), egui::Color32::from_rgba_unmultiplied(255, 255, 255, 180));

                let pad_dwell_id = ui.make_persistent_id(format!("dwell_pad_{idx}"));
                if resp.clicked() || apply_dwell_to_response(ui, dwell_state, pad_dwell_id, &resp) {
                    state.trigger_pad(idx, 0.0);
                    state.active_pad_index = idx;
                }
            }
        });
        ui.add_space(6.0);
    }

    ui.add_space(4.0);

    // 3. Bouton géant de contacteur unique
    let switch_bar_w = ui.available_width();
    let (bar_rect, bar_resp) = ui.allocate_exact_size(egui::vec2(switch_bar_w, 48.0), egui::Sense::drag());

    let is_holding = bar_resp.is_pointer_button_down_on();
    let now = Instant::now();
    if is_holding {
        if state.press_start.is_none() {
            state.press_start = Some(now);
            state.trigger_pad(state.active_pad_index, 0.0);
        }
    } else if let Some(start) = state.press_start.take() {
        let dur = start.elapsed().as_secs_f32();
        if dur > 0.08 {
            state.trigger_pad(state.active_pad_index, dur);
        }
    }

    let hold_dur = state.press_start.map_or(0.0, |s| s.elapsed().as_secs_f32());
    let p = ui.painter();
    let bar_color = if is_holding {
        egui::Color32::from_rgb(0, 210, 255)
    } else {
        theme.tab_active_bg
    };
    p.rect(bar_rect, egui::CornerRadius::same(8), bar_color, egui::Stroke::new(2.0, theme.accent), egui::StrokeKind::Inside);

    let bar_text = if hold_dur > 0.05 {
        let semitones = (hold_dur * 4.0).min(7.0);
        let wah = (state.wah_cutoff * 100.0).min(100.0);
        t!("instrument.morphing_active", pitch = format!("{semitones:.1}"), wah = format!("{wah:.0}")).to_string()
    } else {
        t!("instrument.switch_btn_idle").to_string()
    };
    p.text(bar_rect.center(), egui::Align2::CENTER_CENTER, bar_text, egui::FontId::proportional(12.5), egui::Color32::WHITE);

    ui.add_space(6.0);

    // 4. Curseurs de contrôle et modulation
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(t!("instrument.bpm")).size(11.0).color(theme.text_secondary));
        ui.add(egui::Slider::new(&mut state.bpm, 60..=160).show_value(true));
        ui.separator();
        ui.label(egui::RichText::new(t!("instrument.wah_filter")).size(11.0).color(theme.text_secondary));
        ui.add(egui::Slider::new(&mut state.wah_cutoff, 0.1..=1.0).show_value(false));
        ui.separator();
        ui.label(egui::RichText::new(t!("instrument.pitch")).size(11.0).color(theme.text_secondary));
        ui.add(egui::Slider::new(&mut state.pitch_bend, -12.0..=12.0).show_value(true));
    });

    if let Ok(mut guard) = INSTRUMENT_STATE.lock() {
        *guard = Some(state);
    }
}