use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::types::{AgentEvent, AgentStatus, AudioCommand};

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
        if max_abs < 0.005 {
            return Ok(String::new());
        }

        let mut state = self.ctx.create_state()
            .map_err(|e| format!("Erreur création état Whisper: {e}"))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("fr"));
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

pub fn find_whisper_model_file() -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("WHISPER_MODEL_PATH") {
        let p = PathBuf::from(env_path.trim());
        if p.is_file() {
            return Some(p);
        }
    }

    let candidates = [
        "models/ggml-base.bin",
        "models/ggml-tiny.bin",
        "ggml-base.bin",
        "ggml-tiny.bin",
    ];

    for candidate in &candidates {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }

    None
}

fn ensure_model_downloaded(event_tx: &Sender<AgentEvent>) -> Result<PathBuf, String> {
    if let Some(existing) = find_whisper_model_file() {
        return Ok(existing);
    }

    let target_dir = PathBuf::from("models");
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

fn handle_cpal_stream_err(err: impl std::fmt::Display) {
    let msg = err.to_string().to_lowercase();
    if !msg.contains("overrun") && !msg.contains("underrun") {
        eprintln!("Erreur de capture audio cpal : {err}");
    }
}

pub fn spawn_audio_worker(event_tx: Sender<AgentEvent>, _api_key: String) -> Sender<AudioCommand> {
    let (cmd_tx, cmd_rx) = channel::<AudioCommand>();
    let whisper_instance: Arc<Mutex<Option<WhisperEngine>>> = Arc::new(Mutex::new(None));
    let is_model_ready = Arc::new(AtomicBool::new(false));

    // 1. Thread de chargement asynchrone : ne bloque pas l'affichage de l'interface
    {
        let whisper_instance = Arc::clone(&whisper_instance);
        let is_model_ready = Arc::clone(&is_model_ready);
        let event_tx = event_tx.clone();
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
                        let _ = event_tx.send(AgentEvent::SilentNarration(
                            "✅ Modèle Whisper prêt. Reconnaissance vocale disponible.".into(),
                        ));
                    }
                    Err(err) => {
                        eprintln!("[Whisper] Erreur lors du chargement de Whisper: {err}");
                        let _ = event_tx.send(AgentEvent::SilentNarration(
                            format!("❌ Erreur chargement Whisper : {err}"),
                        ));
                    }
                },
                Err(err) => {
                    eprintln!("[Whisper] Modèle introuvable et échec de téléchargement: {err}");
                    let _ = event_tx.send(AgentEvent::SilentNarration(
                        format!("❌ Modèle Whisper indisponible : {err}"),
                    ));
                }
            }
        });
    }

    // 2. Thread de capture audio : traite les commandes utilisateur
    std::thread::spawn(move || {
        while let Ok(cmd) = cmd_rx.recv() {
            if !matches!(cmd, AudioCommand::Start) {
                continue;
            }

            if !is_model_ready.load(Ordering::SeqCst) {
                let _ = event_tx.send(AgentEvent::SilentNarration(
                    "⏳ Le modèle vocal est en cours de chargement en arrière-plan, veuillez patienter un instant...".into(),
                ));
                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                continue;
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

            let stream = match sample_format {
                cpal::SampleFormat::F32 => device.build_input_stream(
                    stream_config.clone(),
                    move |data: &[f32], _| {
                        let _ = audio_tx.send(data.to_vec());
                    },
                    |err| handle_cpal_stream_err(err),
                    None,
                ).ok(),
                cpal::SampleFormat::I16 => device.build_input_stream(
                    stream_config.clone(),
                    move |data: &[i16], _| {
                        let converted: Vec<f32> =
                            data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                        let _ = audio_tx.send(converted);
                    },
                    |err| handle_cpal_stream_err(err),
                    None,
                ).ok(),
                cpal::SampleFormat::U16 => device.build_input_stream(
                    stream_config,
                    move |data: &[u16], _| {
                        let converted: Vec<f32> =
                            data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0).collect();
                        let _ = audio_tx.send(converted);
                    },
                    |err| handle_cpal_stream_err(err),
                    None,
                ).ok(),
                _ => None,
            };

            let Some(stream) = stream else {
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
            let mut audio_16k_buffer: Vec<f32> = Vec::with_capacity(16000 * 10);
            let mut speech_detected = false;
            let mut last_speech_time = Instant::now();
            let mut start_recording_time = Instant::now();
            const SPEECH_ENERGY_THRESHOLD: f32 = 0.012;
            const MAX_RECORDING_DURATION: Duration = Duration::from_secs(25);

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

                    let mic_level = if rms > 0.003 {
                        ((rms - 0.003) * 15.0).clamp(0.0, 1.0)
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
                        // Pré-tampon de 300 ms pour conserver le début de phrase
                        audio_16k_buffer.extend_from_slice(&new_16k);
                        if audio_16k_buffer.len() > 4800 {
                            let overflow = audio_16k_buffer.len() - 4800;
                            audio_16k_buffer.drain(0..overflow);
                        }
                    }
                }

                if speech_detected {
                    let silence_elapsed = last_speech_time.elapsed();
                    let total_elapsed = start_recording_time.elapsed();

                    let is_dictation = crate::automation::IS_DICTATION_ACTIVE.load(std::sync::atomic::Ordering::Relaxed);
                    let silence_timeout = if is_dictation {
                        Duration::from_millis(2200)
                    } else {
                        Duration::from_millis(1750)
                    };

                    if (silence_elapsed >= silence_timeout && audio_16k_buffer.len() >= 8000)
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
                            Ok(text) if !text.trim().is_empty() => {
                                let _ = event_tx.send(AgentEvent::VoicePromptReady(text));
                                break;
                            }
                            Ok(_) => {
                                speech_detected = false;
                                audio_16k_buffer.clear();
                                let _ = event_tx.send(AgentEvent::TranscriptionPartial(
                                    "Écoute Whisper active...".into(),
                                ));
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