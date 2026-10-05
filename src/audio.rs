use std::io::Cursor;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Deserialize;

use crate::types::{AgentEvent, AgentStatus, AudioCommand};

pub fn encode_wav(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    if let Ok(mut writer) = hound::WavWriter::new(&mut cursor, spec) {
        let ch = channels as usize;
        for chunk in samples.chunks(ch) {
            let mono: f32 = chunk.iter().sum::<f32>() / ch as f32;
            let sample_i16 = (mono.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            let _ = writer.write_sample(sample_i16);
        }
        let _ = writer.finalize();
    }
    cursor.into_inner()
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

/// Filtre adaptatif NLMS (Normalized Least Mean Squares) avec détecteur de double parole (DTD)
/// Modélise la réponse impulsionnelle acoustique de la pièce (~48 ms à 16 kHz = 768 taps)
pub struct NlmsAec {
    taps: usize,
    weights: Vec<f32>,
    x_buf: Vec<f32>,
    x_head: usize,
    step_size: f32,
    x_power: f32,
    d_power: f32,
    e_power: f32,
}

impl NlmsAec {
    pub fn new(taps: usize) -> Self {
        Self {
            taps,
            weights: vec![0.0; taps],
            x_buf: vec![0.0; taps],
            x_head: 0,
            step_size: 0.20,
            x_power: 0.0,
            d_power: 0.0,
            e_power: 0.0,
        }
    }

    pub fn process(&mut self, mic_sample: f32, spk_sample: f32) -> f32 {
        let x = spk_sample;
        let d = mic_sample;

        // Insertion du signal haut-parleur dans le tampon circulaire de référence
        self.x_buf[self.x_head] = x;

        // Estimation lissée des puissances des signaux (alpha = 0.005)
        const ALPHA: f32 = 0.005;
        self.x_power = (1.0 - ALPHA) * self.x_power + ALPHA * (x * x);
        self.d_power = (1.0 - ALPHA) * self.d_power + ALPHA * (d * d);

        // Si aucun son significatif ne sort des haut-parleurs, contourner le filtrage
        if self.x_power < 1e-5 {
            self.x_head = if self.x_head + 1 >= self.taps { 0 } else { self.x_head + 1 };
            return d;
        }

        // Écho estimé y_chapeau = sum(w_i * x_{n-i})
        let mut y_hat: f32 = 0.0;
        let mut norm: f32 = 1e-4;
        let taps = self.taps;
        let head = self.x_head;

        for i in 0..taps {
            let idx = if head >= i { head - i } else { head + taps - i };
            let xi = self.x_buf[idx];
            y_hat += self.weights[i] * xi;
            norm += xi * xi;
        }

        // Signal nettoyé (soustraction de l'écho acoustique estimé)
        let e = d - y_hat;
        self.e_power = (1.0 - ALPHA) * self.e_power + ALPHA * (e * e);

        // Détecteur de double parole (DTD)
        let is_double_talk = self.d_power > 0.0008 && self.e_power > 0.45 * self.d_power;
        if !is_double_talk {
            let adapt = (self.step_size / norm) * e;
            const LEAKAGE: f32 = 0.99998;
            for i in 0..taps {
                let idx = if head >= i { head - i } else { head + taps - i };
                self.weights[i] = self.weights[i] * LEAKAGE + adapt * self.x_buf[idx];
            }
        }

        self.x_head = if self.x_head + 1 >= self.taps { 0 } else { self.x_head + 1 };
        e.clamp(-1.0, 1.0)
    }
}

pub async fn transcribe_audio(
    api_key: &str,
    wav_data: Vec<u8>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let mut attempts = 0;
    loop {
        attempts += 1;
        let part = reqwest::multipart::Part::bytes(wav_data.clone())
            .file_name("speech.wav")
            .mime_str("audio/wav")?;

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", "whisper-large-v3-turbo")
            .text("language", "fr")
            .text("prompt", "Transcription en français uniquement.")
            .text("response_format", "json");

        let res = client
            .post("https://api.groq.com/openai/v1/audio/transcriptions")
            .bearer_auth(api_key)
            .multipart(form)
            .send()
            .await?;

        if res.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempts <= 2 {
            tokio::time::sleep(Duration::from_millis(2500)).await;
            continue;
        }

        if !res.status().is_success() {
            let err = res.text().await.unwrap_or_default();
            return Err(format!("Erreur transcription: {err}").into());
        }

        #[derive(Deserialize)]
        struct TranscribeResp {
            text: String,
        }

        let body = res.json::<TranscribeResp>().await?;
        return Ok(body.text.trim().to_string());
    }
}

pub fn spawn_audio_worker(event_tx: Sender<AgentEvent>, groq_key: String) -> Sender<AudioCommand> {
    let (cmd_tx, cmd_rx) = channel::<AudioCommand>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Échec runtime audio");

        let host = cpal::default_host();

        while let Ok(cmd) = cmd_rx.recv() {
            if !matches!(cmd, AudioCommand::Start) {
                continue;
            }

            if groq_key.is_empty() {
                let _ = event_tx.send(AgentEvent::ReplaceNarration(
                    "Veuillez renseigner GROQ_API_KEY dans le fichier .env pour activer Whisper.".into(),
                ));
                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                continue;
            }

            let Some(device) = host.default_input_device() else {
                let _ = event_tx.send(AgentEvent::ReplaceNarration(
                    "Aucun microphone disponible.".into(),
                ));
                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                continue;
            };

            let Ok(config) = device.default_input_config() else {
                continue;
            };

            let mic_sr = config.sample_rate().0;
            let mic_ch = config.channels();
            let mic_raw_buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
            let mic_buf_clone = mic_raw_buffer.clone();

            let mic_stream_res = match config.sample_format() {
                cpal::SampleFormat::F32 => device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _| {
                        if let Ok(mut b) = mic_buf_clone.lock() {
                            b.extend_from_slice(data);
                        }
                    },
                    |_| {},
                    None,
                ),
                cpal::SampleFormat::I16 => device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _| {
                        if let Ok(mut b) = mic_buf_clone.lock() {
                            for &s in data {
                                b.push(s as f32 / i16::MAX as f32);
                            }
                        }
                    },
                    |_| {},
                    None,
                ),
                _ => continue,
            };

            let Ok(mic_stream) = mic_stream_res else { continue; };
            let _ = mic_stream.play();

            // Capture simultanée en boucle de retour WASAPI sur les haut-parleurs
            let spk_raw_buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
            let (spk_stream, spk_sr, spk_ch) = if let Some(out_dev) = host.default_output_device() {
                let config_res = out_dev.default_output_config().or_else(|_| out_dev.default_input_config());
                if let Ok(spk_conf) = config_res {
                    let sr = spk_conf.sample_rate().0;
                    let ch = spk_conf.channels();
                    let spk_clone = spk_raw_buffer.clone();
                    let s = match spk_conf.sample_format() {
                        cpal::SampleFormat::F32 => out_dev.build_input_stream(
                            &spk_conf.into(),
                            move |data: &[f32], _| {
                                if let Ok(mut b) = spk_clone.lock() {
                                    b.extend_from_slice(data);
                                }
                            },
                            |_| {},
                            None,
                        ).ok(),
                        cpal::SampleFormat::I16 => {
                            let spk_clone = spk_raw_buffer.clone();
                            out_dev.build_input_stream(
                                &spk_conf.into(),
                                move |data: &[i16], _| {
                                    if let Ok(mut b) = spk_clone.lock() {
                                        for &s in data {
                                            b.push(s as f32 / i16::MAX as f32);
                                        }
                                    }
                                },
                                |_| {},
                                None,
                            ).ok()
                        }
                        _ => None,
                    };
                    (s, sr, ch)
                } else {
                    (None, 48000, 2)
                }
            } else {
                (None, 48000, 2)
            };

            if let Some(ref s) = spk_stream {
                let _ = s.play();
            }

            // Rééchantillonneurs 16 kHz et filtre AEC (768 coefficients = ~48 ms)
            const TARGET_SAMPLE_RATE: u32 = 16000;
            let mut mic_resampler = ContinuousResampler::new(mic_sr, mic_ch, TARGET_SAMPLE_RATE);
            let mut spk_resampler = ContinuousResampler::new(spk_sr, spk_ch, TARGET_SAMPLE_RATE);
            let mut aec = NlmsAec::new(768);

            let mut mic_16k = Vec::new();
            let mut spk_16k = Vec::new();
            let mut cleaned_audio = Vec::<f32>::new();

            let mut last_voice_instant = Instant::now();
            let mut has_spoken = false;
            let mut last_interim_instant = Instant::now();
            let mut last_processed_len = 0;

            loop {
                std::thread::sleep(Duration::from_millis(100));
                if let Ok(AudioCommand::Stop) = cmd_rx.try_recv() {
                    break;
                }

                // Dépouillement des échantillons bruts du microphone et des haut-parleurs
                let raw_mic = {
                    let mut lock = mic_raw_buffer.lock().unwrap();
                    std::mem::take(&mut *lock)
                };
                let raw_spk = {
                    let mut lock = spk_raw_buffer.lock().unwrap();
                    std::mem::take(&mut *lock)
                };

                mic_resampler.push_interleaved_f32(&raw_mic);
                spk_resampler.push_interleaved_f32(&raw_spk);

                mic_resampler.drain_resampled(&mut mic_16k);
                spk_resampler.drain_resampled(&mut spk_16k);

                // Si les haut-parleurs n'émettent aucun flux audio, aligner avec des zéros
                while spk_16k.len() < mic_16k.len() {
                    spk_16k.push(0.0);
                }

                let process_count = mic_16k.len();
                for i in 0..process_count {
                    let cleaned = aec.process(mic_16k[i], spk_16k[i]);
                    cleaned_audio.push(cleaned);
                }
                mic_16k.clear();
                spk_16k.drain(0..process_count);

                let current_len = cleaned_audio.len();
                let slice = &cleaned_audio[last_processed_len..];
                let sum_sq: f32 = slice.iter().map(|&x| x * x).sum();
                let recent_rms = if !slice.is_empty() { (sum_sq / slice.len() as f32).sqrt() } else { 0.0 };
                last_processed_len = current_len;

                if recent_rms > 0.015 {
                    last_voice_instant = Instant::now();
                    has_spoken = true;
                }

                // Retranscription intermédiaire en temps réel pendant l'élocution
                if has_spoken && last_interim_instant.elapsed() >= Duration::from_millis(1500) {
                    last_interim_instant = Instant::now();
                    let snapshot = cleaned_audio.clone();
                    if !snapshot.is_empty() {
                        let wav = encode_wav(&snapshot, TARGET_SAMPLE_RATE, 1);
                        let key = groq_key.clone();
                        let tx_clone = event_tx.clone();
                        rt.spawn(async move {
                            if let Ok(text) = transcribe_audio(&key, wav).await {
                                if !text.is_empty() {
                                    let _ = tx_clone.send(AgentEvent::TranscriptionPartial(text));
                                }
                            }
                        });
                    }
                }

                // Si l'utilisateur s'arrête de parler pendant 1,4 s, la consigne est validée
                if has_spoken && last_voice_instant.elapsed() >= Duration::from_millis(1400) {
                    break;
                }
            }

            drop(mic_stream);
            drop(spk_stream);

            if has_spoken && !cleaned_audio.is_empty() {
                let wav = encode_wav(&cleaned_audio, TARGET_SAMPLE_RATE, 1);
                if let Ok(final_text) = rt.block_on(transcribe_audio(&groq_key, wav)) {
                    if !final_text.is_empty() {
                        let _ = event_tx.send(AgentEvent::VoicePromptReady(final_text));
                        continue;
                    }
                }
            }
            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
        }
    });

    cmd_tx
}