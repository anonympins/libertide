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
            .file_name("audio.wav")
            .mime_str("audio/wav")?;

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", "whisper-large-v3")
            .text("language", "fr")
            .text("temperature", "0")
            .text("response_format", "verbose_json");

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
            return Err(format!("Erreur transcription : {err}").into());
        }

        #[derive(Deserialize)]
        struct TranscribeResp {
            #[serde(default)]
            text: String,
        }

        let body = res.json::<TranscribeResp>().await?;
        return Ok(body.text.trim().to_string());
    }
}

pub fn spawn_audio_worker(event_tx: Sender<AgentEvent>, groq_key: String) -> Sender<AudioCommand> {
    let (cmd_tx, cmd_rx) = channel::<AudioCommand>();

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(_) => return,
        };

        let host = cpal::default_host();

        while let Ok(cmd) = cmd_rx.recv() {
            if !matches!(cmd, AudioCommand::Start) {
                continue;
            }

            // La dictée vocale est déléguée à Windows (Win + H) afin de réserver l'intégralité du quota à Groq LLM
            loop {
                std::thread::sleep(Duration::from_millis(150));
                if let Ok(AudioCommand::Stop) = cmd_rx.try_recv() {
                    break;
                }
            }
            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
        }
    });

    cmd_tx
}