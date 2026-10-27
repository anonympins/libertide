use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::automation::IS_DICTATION_ACTIVE;
use crate::types::{AgentEvent, AgentStatus, AudioCommand};

// ============================================================================
// 1. Rééchantillonneur audio continu (vers mono 16 kHz)
// ============================================================================

/// Détecte les bruits parasites, didascalies ou hallucinations fréquentes de Whisper sur le silence
fn is_hallucination_or_noise(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    let clean: String = lower.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect();
    let clean_str = clean.trim();
    let words: Vec<&str> = clean_str.split_whitespace().collect();
    if words.is_empty() {
        return true;
    }
    const EXACT_NOISE_PHRASES: &[&str] = &[
        "merci",
        "merci beaucoup",
        "merci d avoir regarde",
        "merci davoir regarde",
        "merci a tous",
        "merci davoir visionne",
        "sous titres",
        "sous titrage",
        "sous titres realises par",
        "amara org",
        "musique",
        "musique douce",
        "musique de fond",
        "applaudissements",
        "rires",
        "soupir",
        "bruit",
        "bruit de fond",
        "chuchotement",
        "silence",
        "you",
        "thank you",
        "thanks for watching",
        "subtitles",
        "transcription",
        "a bientot",
        "au revoir",
        "bye",
    ];
    let trimmed = text.trim();
    EXACT_NOISE_PHRASES.iter().any(|&p| clean_str == p)
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
        || (trimmed.starts_with('(') && trimmed.ends_with(')'))
        || (trimmed.starts_with('*') && trimmed.ends_with('*'))
        || (trimmed.starts_with('♪') || trimmed.ends_with('♪'))
        || (trimmed.starts_with('♫') || trimmed.ends_with('♫'))
        || trimmed.chars().all(|c| !c.is_alphanumeric())
}

/// Rééchantillonneur linéaire continu vers mono 16 kHz sans dépendance externe
pub struct ContinuousResampler {
    in_rate: f64,
    out_rate: f64,
    channels: usize,
    buffer: Vec<f32>,
    phase: f64,
}

impl ContinuousResampler {
    pub fn new(in_rate: u32, channels: u16, out_rate: u32) -> Self {
        Self {
            in_rate: in_rate as f64,
            out_rate: out_rate as f64,
            channels: (channels as usize).max(1),
            buffer: Vec::with_capacity(4096),
            phase: 0.0,
        }
    }

    pub fn push_interleaved_f32(&mut self, data: &[f32]) {
        let ch = self.channels;
        for frame in data.chunks(ch) {
            let mono: f32 = frame.iter().sum::<f32>() / ch as f32;
            self.buffer.push(mono);
        }
    }

    pub fn drain_resampled(&mut self, out: &mut Vec<f32>) {
        if self.buffer.len() < 2 {
            return;
        }
        let step = self.in_rate / self.out_rate;
        let max_idx = self.buffer.len() - 1;
        while self.phase < max_idx as f64 {
            let idx = self.phase.floor() as usize;
            let frac = (self.phase - idx as f64) as f32;
            let s0 = self.buffer[idx];
            let s1 = self.buffer[idx + 1];
            out.push(s0 + frac * (s1 - s0));
            self.phase += step;
        }
        let consumed = self.phase.floor() as usize;
        if consumed > 0 {
            self.buffer.drain(0..consumed);
            self.phase -= consumed as f64;
        }
    }
}

// ============================================================================
// 2. Moteur d'inférence Whisper local (whisper.cpp)
// ============================================================================

pub struct WhisperEngine {
    ctx: WhisperContext,
}

impl WhisperEngine {
    pub fn load(model_path: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let path_str = model_path.to_str().ok_or("Chemin du modèle introuvable")?;
        let mut params = WhisperContextParameters::default();
        params.use_gpu(true);
        let ctx = WhisperContext::new_with_params(path_str, params)
            .map_err(|e| format!("Erreur initialisation whisper.cpp: {e}"))?;
        Ok(Self { ctx })
    }

    pub fn transcribe(&mut self, samples_16k: &[f32]) -> Result<String, String> {
        if samples_16k.is_empty() {
            return Ok(String::new());
        }

        let max_abs = samples_16k.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        if max_abs < 0.012 {
            return Ok(String::new());
        }

        let mut state = self.ctx.create_state()
            .map_err(|e| format!("Erreur création état Whisper: {e}"))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        let active_locale = rust_i18n::locale();
        let lang_code = active_locale.split(['-', '_']).next().unwrap_or("fr");
        let num_threads = std::thread::available_parallelism()
            .map(|n| (n.get() as i32 / 3).clamp(1, 2))
            .unwrap_or(2);
        params.set_n_threads(num_threads);
        params.set_language(Some(lang_code));
        params.set_translate(false);
        params.set_no_context(true);
        params.set_single_segment(true);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state.full(params, samples_16k)
            .map_err(|e| format!("Erreur inférence whisper.cpp: {e}"))?;

        let num_segments = state.full_n_segments();

        let mut result = String::new();
        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                result.push_str(&segment.to_string());
            }
        }

        Ok(result.trim().to_string())
    }
}

// ============================================================================
// 3. Gestion et téléchargement des modèles GGML Whisper
// ============================================================================

pub fn find_whisper_model_file() -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("WHISPER_MODEL_PATH") {
        let p = PathBuf::from(env_path.trim());
        if p.is_file() {
            return Some(p);
        }
    }

    let model_names = ["ggml-base.bin", "ggml-tiny.bin"];

    // 1. Racine du dépôt via CARGO_MANIFEST_DIR (infaillible lors du dev / cargo run)
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for name in &model_names {
        let p1 = manifest_dir.join("models").join(name);
        if p1.is_file() {
            return Some(p1);
        }
        let p2 = manifest_dir.join(name);
        if p2.is_file() {
            return Some(p2);
        }
    }

    // 2. Dossier de l'exécutable et ses parents (remonte depuis target/debug ou racine d'installation)
    if let Ok(exe) = std::env::current_exe() {
        let mut curr = exe.parent();
        for _ in 0..6 {
            if let Some(dir) = curr {
                for name in &model_names {
                    let p1 = dir.join("models").join(name);
                    if p1.is_file() {
                        return Some(p1);
                    }
                    let p2 = dir.join(name);
                    if p2.is_file() {
                        return Some(p2);
                    }
                }
                curr = dir.parent();
            } else {
                break;
            }
        }
    }

    // 3. Répertoire de travail courant et ses parents
    if let Ok(cwd) = std::env::current_dir() {
        let mut curr = Some(cwd.as_path());
        for _ in 0..5 {
            if let Some(dir) = curr {
                for name in &model_names {
                    let p1 = dir.join("models").join(name);
                    if p1.is_file() {
                        return Some(p1);
                    }
                    let p2 = dir.join(name);
                    if p2.is_file() {
                        return Some(p2);
                    }
                }
                curr = dir.parent();
            } else {
                break;
            }
        }
    }

    None
}

fn get_primary_models_dir() -> PathBuf {
    let manifest_models = Path::new(env!("CARGO_MANIFEST_DIR")).join("models");
    if manifest_models.parent().map_or(false, |p| p.exists()) {
        manifest_models
    } else if let Ok(exe) = std::env::current_exe() {
        let mut curr = exe.parent();
        for _ in 0..6 {
            if let Some(dir) = curr {
                let candidate = dir.join("models");
                if candidate.is_dir() {
                    return candidate;
                }
                curr = dir.parent();
            } else {
                break;
            }
        }
        exe.parent().map(|p| p.join("models")).unwrap_or_else(|| PathBuf::from("models"))
    } else {
        PathBuf::from("models")
    }
}

fn ensure_model_downloaded(event_tx: &Sender<AgentEvent>) -> Result<PathBuf, String> {
    if let Some(existing) = find_whisper_model_file() {
        return Ok(existing);
    }

    let target_dir = get_primary_models_dir();
    let _ = std::fs::create_dir_all(&target_dir);
    let model_path = target_dir.join("ggml-base.bin");

    if model_path.exists() {
        return Ok(model_path);
    }

    let url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin";
    let _ = event_tx.send(AgentEvent::SilentNarration(
        "⏳ Téléchargement du modèle vocal Whisper (148 Mo) en arrière-plan...".into(),
    ));
    println!("[Whisper] Téléchargement de ggml-base.bin depuis Hugging Face...");

    let rt = tokio::runtime::Runtime::new().map_err(|e| format!("Runtime tokio: {e}"))?;
    let client = reqwest::Client::new();
    let resp = rt.block_on(async {
        client.get(url).send().await?.bytes().await
    }).map_err(|e| format!("Échec téléchargement modèle GGML: {e}"))?;

    std::fs::write(&model_path, &resp).map_err(|e| format!("Échec écriture modèle GGML: {e}"))?;
    Ok(model_path)
}

// ============================================================================
// 4. Capture CPAL générique et gestion du flux
// ============================================================================

fn handle_cpal_stream_err(err: impl std::fmt::Display) {
    let msg = err.to_string().to_lowercase();
    if !msg.contains("overrun") && !msg.contains("underrun") {
        eprintln!("[Audio CPAL] Erreur de capture audio : {err}");
    }
}

/// Factorise la création d'un flux d'enregistrement pour les formats F32, I16 et U16
fn build_input_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    audio_tx: Sender<Vec<f32>>,
) -> Option<cpal::Stream> {
    match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config.clone(),
            move |data: &[f32], _| {
                let _ = audio_tx.send(data.to_vec());
            },
            handle_cpal_stream_err,
            None,
        ).ok(),
        cpal::SampleFormat::I16 => device.build_input_stream(
            config.clone(),
            move |data: &[i16], _| {
                let converted: Vec<f32> =
                    data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                let _ = audio_tx.send(converted);
            },
            handle_cpal_stream_err,
            None,
        ).ok(),
        cpal::SampleFormat::I32 => device.build_input_stream(
            config.clone(),
            move |data: &[i32], _| {
                let converted: Vec<f32> = data.iter().map(|&s| s as f32 / i32::MAX as f32).collect();
                let _ = audio_tx.send(converted);
            },
            handle_cpal_stream_err,
            None,
        ).ok(),
        cpal::SampleFormat::U16 => device.build_input_stream(
            config.clone(),
            move |data: &[u16], _| {
                let converted: Vec<f32> =
                    data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0).collect();
                let _ = audio_tx.send(converted);
            },
            handle_cpal_stream_err,
            None,
        ).ok(),
        _ => None,
    }
}

// ============================================================================
// 5. Worker Audio : chargement asynchrone et boucle de traitement VAD
// ============================================================================

pub fn spawn_audio_worker(event_tx: Sender<AgentEvent>, _api_key: String) -> Sender<AudioCommand> {
    let (cmd_tx, cmd_rx) = channel::<AudioCommand>();
    let whisper_instance: Arc<Mutex<Option<WhisperEngine>>> = Arc::new(Mutex::new(None));
    let is_model_ready = Arc::new(AtomicBool::new(false));
    let model_load_failed = Arc::new(AtomicBool::new(false));

    // 1. Thread de chargement asynchrone : ne bloque pas l'affichage de l'interface
    {
        let event_tx = event_tx.clone();
        let whisper_instance = Arc::clone(&whisper_instance);
        let is_model_ready = Arc::clone(&is_model_ready);
        let model_load_failed = Arc::clone(&model_load_failed);
        std::thread::spawn(move || {
            let is_download_needed = find_whisper_model_file().is_none();
            if is_download_needed {
                let _ = event_tx.send(AgentEvent::SilentNarration(
                    "⏳ Modèle vocal Whisper introuvable localement : initialisation du téléchargement...".into(),
                ));
            }

            match ensure_model_downloaded(&event_tx) {
                Ok(path) => match WhisperEngine::load(&path) {
                    Ok(m) => {
                        println!("[Whisper] Modèle Whisper chargé avec succès depuis {}", path.display());
                        *whisper_instance.lock().unwrap() = Some(m);
                        is_model_ready.store(true, Ordering::SeqCst);
                        let _ = event_tx.send(AgentEvent::WhisperReady);
                        let _ = event_tx.send(AgentEvent::SilentNarration(
                            "✅ Modèle Whisper prêt. Reconnaissance vocale disponible.".into(),
                        ));
                    }
                    Err(err) => {
                        model_load_failed.store(true, Ordering::SeqCst);
                        eprintln!("[Whisper] Erreur lors du chargement de Whisper: {err}");
                        let _ = event_tx.send(AgentEvent::SilentNarration(
                            format!("❌ Erreur chargement Whisper : {err}"),
                        ));
                    }
                },
                Err(err) => {
                    model_load_failed.store(true, Ordering::SeqCst);
                    eprintln!("[Whisper] Modèle introuvable et échec de téléchargement: {err}");
                    let _ = event_tx.send(AgentEvent::SilentNarration(
                        format!("❌ Modèle Whisper indisponible : {err}"),
                    ));
                }
            }
        });
    }

    // 3. Thread de sous-titrage en direct (Loopback audio du système / fenêtres applicatives)
    {
        let event_tx = event_tx.clone();
        let whisper_instance = Arc::clone(&whisper_instance);
        let is_model_ready = Arc::clone(&is_model_ready);
        std::thread::Builder::new()
            .name("live_subtitles_worker".to_string())
            .spawn(move || {
                run_live_subtitles_loop(event_tx, whisper_instance, is_model_ready);
            })
            .expect("Échec lancement worker live_subtitles");
    }

    // 2. Thread de capture audio : traite les commandes utilisateur
    std::thread::spawn(move || {
        while let Ok(cmd) = cmd_rx.recv() {
            if !matches!(cmd, AudioCommand::Start) {
                continue;
            }

            // Résolution de la race condition : attente active si le modèle est encore en chargement
            if !is_model_ready.load(Ordering::SeqCst) {
                let _ = event_tx.send(AgentEvent::SilentNarration(
                    "⏳ Initialisation du modèle vocal en cours, mise en attente de l'écoute...".into(),
                ));
                let mut cancelled = false;
                while !is_model_ready.load(Ordering::SeqCst) {
                    if model_load_failed.load(Ordering::SeqCst) {
                        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                        cancelled = true;
                        break;
                    }
                    if let Ok(next_cmd) = cmd_rx.try_recv() {
                        if matches!(next_cmd, AudioCommand::Stop) {
                            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            cancelled = true;
                            break;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                if cancelled {
                    continue;
                }
            }

            let host = cpal::default_host();
            let device = match host.default_input_device() {
                Some(d) => d,
                None => {
                    let _ = event_tx.send(AgentEvent::TranscriptionPartial(
                        "Aucun microphone d'entrée détecté par le système audio.".into(),
                    ));
                    let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                    continue;
                }
            };

            let config = match device.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    let _ = event_tx.send(AgentEvent::TranscriptionPartial(format!(
                        "Erreur configuration audio micro : {e}"
                    )));
                    let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                    continue;
                }
            };

            let sample_rate = config.sample_rate();
            let channels = config.channels();
            let sample_format = config.sample_format();
            let stream_config: cpal::StreamConfig = config.into();
            let (audio_tx, audio_rx) = channel::<Vec<f32>>();

            let Some(stream) = build_input_stream(&device, &stream_config, sample_format, audio_tx) else {
                let _ = event_tx.send(AgentEvent::TranscriptionPartial(
                    "Format de capture microphone non pris en charge.".into(),
                ));
                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                continue;
            };

            if stream.play().is_err() {
                let _ = event_tx.send(AgentEvent::TranscriptionPartial(
                    "Impossible de lancer le flux d'enregistrement audio.".into(),
                ));
                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                continue;
            }

            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Listening));
            let mut resampler = ContinuousResampler::new(sample_rate, channels, 16000);
            let mut audio_16k_buffer: Vec<f32> = Vec::with_capacity(16000 * 40);
            let mut speech_detected = false;
            let mut last_speech_time = Instant::now();
            let mut start_recording_time = Instant::now();
            // Seuil d'énergie calibré pour filtrer les souffles et le bruit de fond
            const SPEECH_ENERGY_THRESHOLD: f32 = 0.012;
            const PRE_ROLL_SAMPLES: usize = 8000; // 500 ms à 16 kHz pour sécuriser l'attaque
            const MAX_RECORDING_DURATION: Duration = Duration::from_secs(40);

            let is_listening = true;
            while is_listening {
                if let Ok(AudioCommand::Stop) = cmd_rx.try_recv() {
                    let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                    crate::set_mic_audio_level(0.0);
                    break;
                }

                while let Ok(chunk) = audio_rx.try_recv() {
                    resampler.push_interleaved_f32(&chunk);
                }

                let mut new_16k = Vec::new();
                resampler.drain_resampled(&mut new_16k);

                if !new_16k.is_empty() {
                    let sum_sq: f32 = new_16k.iter().map(|&s| s * s).sum();
                    let rms = (sum_sq / new_16k.len() as f32).sqrt();

                    let mic_level = if rms > 0.004 {
                        ((rms - 0.004) * 16.0).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    crate::set_mic_audio_level(mic_level);

                    if rms > SPEECH_ENERGY_THRESHOLD {
                        if !speech_detected {
                            speech_detected = true;
                            start_recording_time = Instant::now();
                            let _ = event_tx.send(AgentEvent::TranscriptionPartial(
                                "Écoute de votre voix...".into(),
                            ));
                        }
                        last_speech_time = Instant::now();
                    }

                    if speech_detected {
                        audio_16k_buffer.extend_from_slice(&new_16k);
                    } else {
                        // Pré-tampon de 500 ms pour conserver le début de phrase
                        audio_16k_buffer.extend_from_slice(&new_16k);
                        if audio_16k_buffer.len() > PRE_ROLL_SAMPLES {
                            let overflow = audio_16k_buffer.len() - PRE_ROLL_SAMPLES;
                            audio_16k_buffer.drain(0..overflow);
                        }
                    }
                }

                if speech_detected {
                    let silence_elapsed = last_speech_time.elapsed();
                    let total_elapsed = start_recording_time.elapsed();

                    let is_dictation = IS_DICTATION_ACTIVE.load(Ordering::Relaxed);
                    let silence_timeout = if is_dictation {
                        Duration::from_millis(3200)
                    } else {
                        Duration::from_millis(2400)
                    };

                    if (silence_elapsed >= silence_timeout && audio_16k_buffer.len() >= 12000)
                        || total_elapsed >= MAX_RECORDING_DURATION
                    {
                        crate::set_mic_audio_level(0.0);
                        let _ = event_tx.send(AgentEvent::TranscriptionPartial(
                            "Transcription locale en cours...".into(),
                        ));

                        let transcribe_res = {
                            let mut guard = whisper_instance.lock().unwrap();
                            if let Some(ref mut whisper) = *guard {
                                whisper.transcribe(&audio_16k_buffer)
                            } else {
                                Err("Moteur Whisper non disponible.".to_string())
                            }
                        };

                        match transcribe_res {
                            Ok(text) if !text.trim().is_empty()
                                && text.chars().any(|c| c.is_alphanumeric())
                                && !is_hallucination_or_noise(&text) => {
                                let _ = event_tx.send(AgentEvent::VoicePromptReady(text));
                                break;
                            }
                            Ok(_) => {
                                speech_detected = false;
                                audio_16k_buffer.clear();
                                let _ = event_tx.send(AgentEvent::TranscriptionPartial(String::new()));
                            }
                            Err(err) => {
                                eprintln!("[Whisper] Erreur de transcription : {err}");
                                speech_detected = false;
                                audio_16k_buffer.clear();
                            }
                        }
                    }
                }

                std::thread::sleep(Duration::from_millis(20));
            }
            crate::set_mic_audio_level(0.0);
        }
    });
    cmd_tx
}

// ============================================================================
// 6. Gestion du sous-titrage en direct (Live Subtitles)
// ============================================================================

pub static LIVE_SUBTITLES_RUNNING: AtomicBool = AtomicBool::new(false);
pub static LIVE_SUBTITLES_TARGET: Mutex<Option<String>> = Mutex::new(None);

pub fn start_live_subtitles(target_window: Option<String>) {
    LIVE_SUBTITLES_RUNNING.store(true, Ordering::SeqCst);
    if let Ok(mut lock) = LIVE_SUBTITLES_TARGET.lock() {
        *lock = target_window;
    }
}

pub fn stop_live_subtitles() {
    LIVE_SUBTITLES_RUNNING.store(false, Ordering::SeqCst);
    if let Ok(mut lock) = LIVE_SUBTITLES_TARGET.lock() {
        *lock = None;
    }
}

pub fn is_live_subtitles_active() -> bool {
    LIVE_SUBTITLES_RUNNING.load(Ordering::SeqCst)
}

const FREQ_BANDS: [f32; 16] = [
    80.0, 150.0, 250.0, 400.0, 600.0, 850.0, 1200.0, 1600.0,
    2100.0, 2700.0, 3400.0, 4200.0, 5200.0, 6400.0, 7800.0, 9500.0,
];

fn compute_spectrum_16(samples: &[f32], sample_rate: f32, current: &mut [f32; 16]) {
    if samples.len() < 128 || sample_rate <= 0.0 {
        for b in current.iter_mut() {
            *b *= 0.82;
        }
        return;
    }
    let n = samples.len().min(512);
    let slice = &samples[samples.len() - n..];
    for (i, &center_f) in FREQ_BANDS.iter().enumerate() {
        let omega = 2.0 * std::f32::consts::PI * center_f / sample_rate;
        let mut sum_cos = 0.0f32;
        let mut sum_sin = 0.0f32;
        for (idx, &s) in slice.iter().enumerate() {
            let val = if s.is_finite() { s } else { 0.0 };
            let w = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * idx as f32 / n as f32).cos());
            let ws = val * w;
            let angle = omega * idx as f32;
            sum_cos += ws * angle.cos();
            sum_sin += ws * angle.sin();
        }
        let mag = (sum_cos * sum_cos + sum_sin * sum_sin).sqrt() / (n as f32 * 0.15);
        let band_boost = 1.0 + (i as f32 * 0.18);
        let target = (mag * 2.5 * band_boost).clamp(0.0, 1.0);
        if target > current[i] {
            current[i] += (target - current[i]) * 0.65;
        } else {
            current[i] += (target - current[i]) * 0.25;
        }
    }
}

fn run_live_subtitles_loop(
    event_tx: Sender<AgentEvent>,
    whisper_instance: Arc<Mutex<Option<WhisperEngine>>>,
    is_model_ready: Arc<AtomicBool>,
) {
    while !is_model_ready.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100));
    }

    let (transcribe_tx, transcribe_rx) = channel::<Vec<f32>>();
    let is_transcribing = Arc::new(AtomicBool::new(false));
    let current_sub_text = Arc::new(Mutex::new(String::new()));

    // Worker dédié à l'inférence Whisper pour ne jamais bloquer la capture ni l'égaliseur
    {
        let whisper_instance = Arc::clone(&whisper_instance);
        let is_transcribing = Arc::clone(&is_transcribing);
        let current_sub_text = Arc::clone(&current_sub_text);
        let event_tx = event_tx.clone();
        std::thread::Builder::new()
            .name("live_subtitles_inference".to_string())
            .spawn(move || {
                while let Ok(samples) = transcribe_rx.recv() {
                    let transcribe_res = {
                        let mut guard = whisper_instance.lock().unwrap();
                        if let Some(ref mut whisper) = *guard {
                            whisper.transcribe(&samples)
                        } else {
                            Err("Moteur Whisper non disponible.".to_string())
                        }
                    };
                    if let Ok(text) = transcribe_res {
                        let trimmed = text.trim();
                        if !trimmed.is_empty()
                            && trimmed.chars().any(|c| c.is_alphanumeric())
                            && !is_hallucination_or_noise(trimmed)
                        {
                            if let Ok(mut lock) = current_sub_text.lock() {
                                *lock = trimmed.to_string();
                            }
                        }
                    }
                    is_transcribing.store(false, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(60));
                }
            })
            .expect("Échec lancement worker live_subtitles_inference");
    }

    loop {
        if !LIVE_SUBTITLES_RUNNING.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(80));
            continue;
        }

        #[cfg(windows)]
        {
            if let Ok(mut lock) = LIVE_SUBTITLES_TARGET.lock() {
                if lock.is_none() {
                    let user_wins = crate::window_util::list_user_windows();
                    let found = user_wins.iter()
                        .find(|(_, t)| {
                            let low = t.to_lowercase();
                            low.contains("youtube")
                                || low.contains("spotify")
                                || low.contains("twitch")
                                || low.contains("netflix")
                                || low.contains("deezer")
                                || low.contains("vlc")
                        })
                        .map(|(_, t)| t.clone())
                        .or_else(|| {
                            crate::window_util::find_best_browser_window(&user_wins, None)
                                .and_then(|h| user_wins.iter().find(|(win_h, _)| *win_h == h).map(|(_, t)| t.clone()))
                        });
                    if let Some(src) = found {
                        *lock = Some(src);
                    }
                }
            }
        }

        // Construction résiliente du flux audio : tentative loopback sortie, sinon repli sur l'entrée
        let host = cpal::default_host();
        let (loopback_tx, loopback_rx) = channel::<Vec<f32>>();
        let mut stream_opt = None;
        let mut sample_rate = 44100;
        let mut channels = 2;

        if let Some(out_dev) = host.default_output_device() {
            if let Ok(config) = out_dev.default_output_config() {
                let sr = config.sample_rate();
                let ch = config.channels();
                let fmt = config.sample_format();
                let s_cfg: cpal::StreamConfig = config.into();
                if let Some(s) = build_input_stream(&out_dev, &s_cfg, fmt, loopback_tx.clone()) {
                    if s.play().is_ok() {
                        stream_opt = Some(s);
                        sample_rate = sr;
                        channels = ch;
                    }
                }
            }
        }

        if stream_opt.is_none() {
            if let Some(in_dev) = host.default_input_device() {
                if let Ok(config) = in_dev.default_input_config() {
                    let sr = config.sample_rate();
                    let ch = config.channels();
                    let fmt = config.sample_format();
                    let s_cfg: cpal::StreamConfig = config.into();
                    if let Some(s) = build_input_stream(&in_dev, &s_cfg, fmt, loopback_tx.clone()) {
                        if s.play().is_ok() {
                            stream_opt = Some(s);
                            sample_rate = sr;
                            channels = ch;
                        }
                    }
                }
            }
        }

        let Some(stream) = stream_opt else {
            eprintln!("[Live Subtitles] Aucun flux audio disponible.");
            std::thread::sleep(Duration::from_millis(500));
            continue;
        };

        println!("[Live Subtitles] Flux de capture loopback audio actif ({} Hz, {} canaux).", sample_rate, channels);

        let mut resampler = ContinuousResampler::new(sample_rate, channels, 16000);
        let mut raw_mono_buffer: Vec<f32> = Vec::with_capacity(1024);
        let mut audio_16k_buffer: Vec<f32> = Vec::with_capacity(16000 * 10);
        let mut current_spectrum = [0.0f32; 16];
        let mut speech_detected = false;
        let mut last_sound_time = Instant::now();
        let mut speech_start = Instant::now();
        let mut last_spectrum_send = Instant::now();
        let mut last_transcribe_time = Instant::now();

        while LIVE_SUBTITLES_RUNNING.load(Ordering::SeqCst) {
            while let Ok(chunk) = loopback_rx.try_recv() {
                let ch = channels as usize;
                for frame in chunk.chunks(ch) {
                    let mono: f32 = frame.iter().sum::<f32>() / ch as f32;
                    let clean_mono = if mono.is_finite() { mono } else { 0.0 };
                    raw_mono_buffer.push(clean_mono);
                }
                if raw_mono_buffer.len() > 1024 {
                    let overflow = raw_mono_buffer.len() - 1024;
                    raw_mono_buffer.drain(0..overflow);
                }
                resampler.push_interleaved_f32(&chunk);
            }

            compute_spectrum_16(&raw_mono_buffer, sample_rate as f32, &mut current_spectrum);

            if last_spectrum_send.elapsed() >= Duration::from_millis(50) {
                last_spectrum_send = Instant::now();
                let text = current_sub_text.lock().map(|g| g.clone()).unwrap_or_default();
                let _ = event_tx.send(AgentEvent::LiveSubtitleUpdate {
                    text,
                    spectrum: current_spectrum,
                });
            }

            let mut new_16k = Vec::new();
            resampler.drain_resampled(&mut new_16k);

            if !new_16k.is_empty() {
                let sum_sq: f32 = new_16k.iter().map(|&s| s * s).sum();
                let rms = (sum_sq / new_16k.len() as f32).sqrt();

                if rms > 0.007 {
                    if !speech_detected {
                        speech_detected = true;
                        speech_start = Instant::now();
                    }
                    last_sound_time = Instant::now();
                    audio_16k_buffer.extend_from_slice(&new_16k);
                } else if speech_detected {
                    audio_16k_buffer.extend_from_slice(&new_16k);
                }
            }

            if speech_detected {
                let silence_dur = last_sound_time.elapsed();
                let total_dur = speech_start.elapsed();

                let has_natural_pause = silence_dur >= Duration::from_millis(650) && audio_16k_buffer.len() >= 14000;
                let max_duration_reached = total_dur >= Duration::from_millis(4200) && audio_16k_buffer.len() >= 16000;

                // Cooldown d'au moins 1,2 seconde pour laisser respirer le processeur
                if (has_natural_pause || max_duration_reached)
                    && !is_transcribing.load(Ordering::SeqCst)
                    && last_transcribe_time.elapsed() >= Duration::from_millis(1200)
                {
                    last_transcribe_time = Instant::now();
                    // Ne pas dépasser 4 secondes d'audio transmises d'un coup à Whisper
                    let transcribe_buf = if audio_16k_buffer.len() > 16000 * 4 {
                        audio_16k_buffer[audio_16k_buffer.len() - (16000 * 4)..].to_vec()
                    } else {
                        audio_16k_buffer.clone()
                    };

                    if has_natural_pause {
                        speech_detected = false;
                        audio_16k_buffer.clear();
                    } else {
                        let keep = 5600.min(audio_16k_buffer.len());
                        let drain_len = audio_16k_buffer.len() - keep;
                        audio_16k_buffer.drain(0..drain_len);
                        speech_start = Instant::now();
                    }

                    is_transcribing.store(true, Ordering::SeqCst);
                    let _ = transcribe_tx.send(transcribe_buf);
                } else if audio_16k_buffer.len() > 16000 * 6 {
                    // Purge proactive pour éviter les longs décalages
                    let overflow = audio_16k_buffer.len() - (16000 * 4);
                    audio_16k_buffer.drain(0..overflow);
                    speech_start = Instant::now();
                }
            }

            std::thread::sleep(Duration::from_millis(15));
        }

        drop(stream);
        if let Ok(mut lock) = current_sub_text.lock() {
            lock.clear();
        }
        let _ = event_tx.send(AgentEvent::LiveSubtitleUpdate {
            text: String::new(),
            spectrum: [0.0; 16],
        });
        println!("[Live Subtitles] Flux de capture loopback arrêté.");
    }
}