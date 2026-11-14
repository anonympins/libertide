use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use eframe::egui;
use serde::{Deserialize, Serialize};
use crate::theme::ThemeColors;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType, Resolution};
use nokhwa::{query, Camera};
use crate::fs_util::{read_env_var_from_file, write_env_var_to_file};
use crate::types::{AgentEvent, NumberedElementTarget};

#[cfg(windows)]
extern "system" {
    fn GetAsyncKeyState(vKey: i32) -> i16;
}

pub static ESCAPE_PRESSED: AtomicBool = AtomicBool::new(false);

/// Indique si la touche Échap (VK_ESCAPE = 0x1B) est actuellement pressée physiquement
#[inline]
pub fn is_escape_key_pressed() -> bool {
    #[cfg(windows)]
    unsafe {
        (GetAsyncKeyState(0x1B) as u16 & 0x8000) != 0
    }
    #[cfg(not(windows))]
    {
        ESCAPE_PRESSED.load(Ordering::Relaxed)
    }
}

/// Vérifie si la touche Échap est pressée et désactive immédiatement l'eye-tracking et la calibration
pub fn check_and_stop_eye_tracking_on_escape() -> bool {
    if is_escape_key_pressed() && (is_eye_tracking_enabled() || is_calibration_active()) {
        set_eye_tracking_enabled(false);
        set_calibration_active(false);
        crate::overlay::play_earcon(crate::overlay::Earcon::Navigation);
        true
    } else {
        false
    }
}

// ============================================================================
// Source de Suivi Oculaire (Webcam IA vs Hardware Eye Trackers)
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EyeTrackerSource {
    /// Webcam RGB standard + réseau de neurones L2CS-Net ONNX (zéro matériel requis)
    WebcamNeural,
    /// Hardware OpenGaze : Tobii Bridge, GazePoint GP3, Pupil Labs, The Eye Tribe (TCP port 4242)
    OpenGazeTcp,
    /// Hardware OpenTrack / FreeTrack / TrackIR bridge (UDP port 4242)
    OpenTrackUdp,
}

pub static EYE_TRACKER_SOURCE: AtomicU32 = AtomicU32::new(0);
pub static EYE_HARDWARE_CONNECTED: AtomicBool = AtomicBool::new(false);
pub static EYE_HARDWARE_HZ: AtomicU32 = AtomicU32::new(0);
pub static EYE_HARDWARE_ADDR: Mutex<String> = Mutex::new(String::new());

pub fn get_eye_tracker_source() -> EyeTrackerSource {
    match EYE_TRACKER_SOURCE.load(Ordering::Relaxed) {
        1 => EyeTrackerSource::OpenGazeTcp,
        2 => EyeTrackerSource::OpenTrackUdp,
        _ => EyeTrackerSource::WebcamNeural,
    }
}

pub fn set_eye_tracker_source(source: EyeTrackerSource) {
    let val = match source {
        EyeTrackerSource::WebcamNeural => 0,
        EyeTrackerSource::OpenGazeTcp => 1,
        EyeTrackerSource::OpenTrackUdp => 2,
    };
    let prev = EYE_TRACKER_SOURCE.swap(val, Ordering::SeqCst);
    if prev != val {
        let name = match source {
            EyeTrackerSource::WebcamNeural => "webcam",
            EyeTrackerSource::OpenGazeTcp => "opengaze",
            EyeTrackerSource::OpenTrackUdp => "opentrack",
        };
        let _ = write_env_var_to_file(".env", "EYE_TRACKER_SOURCE", name);
        std::env::set_var("EYE_TRACKER_SOURCE", name);
        // Redémarrer le worker de capture sur la nouvelle source matérielle
        stop_camera_worker();
        ensure_camera_worker_running();
    }
}

pub fn is_eye_hardware_connected() -> bool {
    EYE_HARDWARE_CONNECTED.load(Ordering::Relaxed)
}

pub fn get_eye_hardware_hz() -> u32 {
    EYE_HARDWARE_HZ.load(Ordering::Relaxed)
}

pub fn get_eye_hardware_addr() -> String {
    EYE_HARDWARE_ADDR.lock().map(|g| g.clone()).unwrap_or_else(|_| "127.0.0.1:4242".to_string())
}

pub fn set_eye_hardware_addr(addr: impl Into<String>) {
    if let Ok(mut g) = EYE_HARDWARE_ADDR.lock() {
        *g = addr.into();
    }
}

// ============================================================================
// i. État Global & Mesures Brutes du Suivi Oculaire
// ============================================================================

/// Échantillon oculaire brut en coordonnées normalisées [-1.0, 1.0]
#[derive(Clone, Copy, Debug)]
pub struct RawGazeMeasurement {
    pub eye_x: f32,
    pub eye_y: f32,
    pub head_yaw: f32,
    pub head_pitch: f32,
    pub timestamp: Instant,
}

impl Default for RawGazeMeasurement {
    fn default() -> Self {
        Self {
            eye_x: 0.0,
            eye_y: 0.0,
            head_yaw: 0.0,
            head_pitch: 0.0,
            timestamp: Instant::now(),
        }
    }
}

pub static EYE_TRACKING_ENABLED: AtomicBool = AtomicBool::new(false);
pub static CAMERA_WORKER_RUNNING: AtomicBool = AtomicBool::new(false);
pub static CALIBRATION_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static EYE_SNAPPING_ENABLED: AtomicBool = AtomicBool::new(true);
pub static EYE_TRACKING_SENSITIVITY: AtomicU32 = AtomicU32::new(1.0f32.to_bits());
pub static EYE_OFFSET_X: AtomicU32 = AtomicU32::new((0f32).to_bits());
pub static EYE_OFFSET_Y: AtomicU32 = AtomicU32::new((0.0f32).to_bits());
pub static EYE_CAMERA_PITCH_OFFSET: AtomicU32 = AtomicU32::new((-16.0f32).to_bits());

pub fn get_camera_pitch_offset() -> f32 {
    f32::from_bits(EYE_CAMERA_PITCH_OFFSET.load(Ordering::Relaxed))
}

pub fn set_camera_pitch_offset(offset: f32) {
    let clamped = offset.clamp(-60.0, 30.0);
    EYE_CAMERA_PITCH_OFFSET.store(clamped.to_bits(), Ordering::Relaxed);
    let val_str = format!("{:.1}", clamped);
    let _ = write_env_var_to_file(".env", "EYE_CAMERA_PITCH_OFFSET", &val_str);
    std::env::set_var("EYE_CAMERA_PITCH_OFFSET", val_str);
}

/// Tare instantanée du pitch caméra sur le regard neutre actuel
pub fn tare_camera_pitch_to_current_gaze() {
    if let Some(raw) = get_latest_raw_gaze() {
        let current_offset = get_camera_pitch_offset();
        let new_offset = current_offset - raw.head_pitch;
        set_camera_pitch_offset(new_offset);
        crate::overlay::play_earcon(crate::overlay::Earcon::Success);
    }
}
pub static EYE_INVERT_X: AtomicBool = AtomicBool::new(false);
pub static EYE_INVERT_Y: AtomicBool = AtomicBool::new(false);
pub static EYE_CONTINUOUS_MODE: AtomicBool = AtomicBool::new(true);
pub static EYE_JOYSTICK_THRESHOLD_A: AtomicU32 = AtomicU32::new(0.025f32.to_bits());

pub fn get_eye_joystick_threshold_a() -> f32 {
    let val = f32::from_bits(EYE_JOYSTICK_THRESHOLD_A.load(Ordering::Relaxed));
    if val < 0.005 { 0.025 } else { val }
}

pub fn set_eye_joystick_threshold_a(threshold: f32) {
    let clamped = threshold.clamp(0.005, 0.200);
    EYE_JOYSTICK_THRESHOLD_A.store(clamped.to_bits(), Ordering::Relaxed);
    let val_str = format!("{:.4}", clamped);
    let _ = write_env_var_to_file(".env", "EYE_JOYSTICK_THRESHOLD_A", &val_str);
    std::env::set_var("EYE_JOYSTICK_THRESHOLD_A", val_str);
}

pub fn is_eye_invert_y() -> bool {
    EYE_INVERT_Y.load(Ordering::Relaxed)
}

pub fn set_eye_invert_y(invert: bool) {
    EYE_INVERT_Y.store(invert, Ordering::Relaxed);
    let val_str = if invert { "true" } else { "false" };
    let _ = write_env_var_to_file(".env", "EYE_INVERT_Y", val_str);
    std::env::set_var("EYE_INVERT_Y", val_str);
}

pub fn is_eye_continuous_mode() -> bool {
    EYE_CONTINUOUS_MODE.load(Ordering::Relaxed)
}

pub fn set_eye_continuous_mode(continuous: bool) {
    EYE_CONTINUOUS_MODE.store(continuous, Ordering::Relaxed);
    let val_str = if continuous { "true" } else { "false" };
    let _ = write_env_var_to_file(".env", "EYE_CONTINUOUS_MODE", val_str);
    std::env::set_var("EYE_CONTINUOUS_MODE", val_str);
}

pub fn is_eye_invert_x() -> bool {
    EYE_INVERT_X.load(Ordering::Relaxed)
}

pub fn set_eye_invert_x(invert: bool) {
    EYE_INVERT_X.store(invert, Ordering::Relaxed);
    let val_str = if invert { "true" } else { "false" };
    let _ = write_env_var_to_file(".env", "EYE_INVERT_X", val_str);
    std::env::set_var("EYE_INVERT_X", val_str);
}

pub fn get_eye_offset_x() -> f32 {
    f32::from_bits(EYE_OFFSET_X.load(Ordering::Relaxed))
}

pub fn set_eye_offset_x(offset: f32) {
    EYE_OFFSET_X.store(offset.to_bits(), Ordering::Relaxed);
    let val_str = format!("{:.1}", offset);
    let _ = write_env_var_to_file(".env", "EYE_OFFSET_X", &val_str);
    std::env::set_var("EYE_OFFSET_X", val_str);
}

pub fn get_eye_offset_y() -> f32 {
    f32::from_bits(EYE_OFFSET_Y.load(Ordering::Relaxed))
}

pub fn set_eye_offset_y(offset: f32) {
    EYE_OFFSET_Y.store(offset.to_bits(), Ordering::Relaxed);
    let val_str = format!("{:.1}", offset);
    let _ = write_env_var_to_file(".env", "EYE_OFFSET_Y", &val_str);
    std::env::set_var("EYE_OFFSET_Y", val_str);
}

pub fn get_eye_tracking_sensitivity() -> f32 {
    f32::from_bits(EYE_TRACKING_SENSITIVITY.load(Ordering::Relaxed))
}

pub fn set_eye_tracking_sensitivity(sensitivity: f32) {
    let clamped = sensitivity.clamp(0.20, 2.00);
    EYE_TRACKING_SENSITIVITY.store(clamped.to_bits(), Ordering::Relaxed);
    let val_str = format!("{:.2}", clamped);
    let _ = write_env_var_to_file(".env", "EYE_TRACKING_SENSITIVITY", &val_str);
    std::env::set_var("EYE_TRACKING_SENSITIVITY", val_str);
}

pub fn is_eye_snapping_enabled() -> bool {
    EYE_SNAPPING_ENABLED.load(Ordering::Relaxed)
}

pub fn set_eye_snapping_enabled(enabled: bool) {
    EYE_SNAPPING_ENABLED.store(enabled, Ordering::Relaxed);
    let _ = write_env_var_to_file(".env", "EYE_SNAPPING_ENABLED", if enabled { "true" } else { "false" });
}

pub fn is_calibration_active() -> bool {
    CALIBRATION_ACTIVE.load(Ordering::Relaxed)
}

pub fn set_calibration_active(active: bool) {
    CALIBRATION_ACTIVE.store(active, Ordering::Relaxed);
}
pub static CAMERA_DEVICE_NAME: Mutex<String> = Mutex::new(String::new());
pub static LATEST_RAW_GAZE: Mutex<Option<RawGazeMeasurement>> = Mutex::new(None);
pub static GLOBAL_GAZE_POS: Mutex<Option<(f32, f32)>> = Mutex::new(None);
static CAMERA_WORKER_HANDLE: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
static CAMERA_START_LOCK: Mutex<()> = Mutex::new(());
static L2CS_SESSION: Mutex<Option<ort::session::Session>> = Mutex::new(None);
pub static DEV_MODE: AtomicBool = AtomicBool::new(false);
pub static SHOW_EYE_DEBUG_OVERLAY: AtomicBool = AtomicBool::new(false);

pub fn set_show_eye_debug_overlay(show: bool) {
    let prev = SHOW_EYE_DEBUG_OVERLAY.swap(show, Ordering::SeqCst);
    if prev == show {
        return;
    }
    if show {
        ensure_camera_worker_running();
    } else if !is_dev_mode() && !is_eye_tracking_enabled() && !is_calibration_active() {
        stop_camera_worker();
    }
}

pub fn is_eye_debug_overlay_visible() -> bool {
    SHOW_EYE_DEBUG_OVERLAY.load(Ordering::Relaxed)
}

/// Indique si le mode développement/débogage est actif (via flag ou argument CLI --dev)
pub fn is_dev_mode() -> bool {
    DEV_MODE.load(Ordering::Relaxed)
        || std::env::args().any(|a| a == "--dev" || a == "-d" || a == "--debug")
}

pub fn set_dev_mode(dev: bool) {
    DEV_MODE.store(dev, Ordering::Relaxed);
    if dev {
        set_show_eye_debug_overlay(true);
        ensure_camera_worker_running();
    }
}

/// Indique si l'overlay de débogage des yeux doit être affiché à l'écran
pub fn should_show_eye_debug() -> bool {
    if (is_dev_mode() || is_eye_tracking_enabled()) && !is_calibration_active() && is_eye_debug_overlay_visible() {
        ensure_camera_worker_running();
        true
    } else {
        false
    }
}
pub static EYE_DEBUG_STATE: Mutex<Option<EyeDebugState>> = Mutex::new(None);

/// État de prévisualisation et de cadrage pour l'overlay de calibration
#[derive(Clone, Debug)]
pub struct EyeDebugState {
    /// Buffer RGBA 8-bit de la vignette recadrée sur les yeux avec annotations
    pub preview_crop: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub left_pupil: Option<(f32, f32)>,
    pub right_pupil: Option<(f32, f32)>,
    pub left_box: (usize, usize, usize, usize),
    pub right_box: (usize, usize, usize, usize),
    pub timestamp: Instant,
}

impl EyeDebugState {
    #[inline]
    pub fn left_eye(&self) -> Option<(f32, f32)> {
        self.left_pupil
    }
    #[inline]
    pub fn right_eye(&self) -> Option<(f32, f32)> {
        self.right_pupil
    }
}

pub fn get_eye_debug_state() -> Option<EyeDebugState> {
    EYE_DEBUG_STATE.lock().ok().and_then(|guard| guard.clone())
}

pub fn set_eye_debug_state(state: EyeDebugState) {
    if let Ok(mut guard) = EYE_DEBUG_STATE.lock() {
        *guard = Some(state);
    }
}
pub static NEURAL_MODEL_LOADED: AtomicBool = AtomicBool::new(false);

pub fn is_eye_tracking_enabled() -> bool {
    EYE_TRACKING_ENABLED.load(Ordering::Relaxed)
}

pub fn set_eye_tracking_enabled(enabled: bool) {
    let prev = EYE_TRACKING_ENABLED.swap(enabled, Ordering::SeqCst);
    if prev == enabled {
        return;
    }
    let _ = write_env_var_to_file(".env", "EYE_TRACKING_ENABLED", if enabled { "true" } else { "false" });
    if enabled {
        if is_dev_mode() {
            set_show_eye_debug_overlay(true);
        }
        ensure_camera_worker_running();
    } else {
        // Ne pas arrêter le worker si le mode dev, la calibration ou l'encadré debug ont besoin de la caméra
        if !is_dev_mode() && !is_calibration_active() && !is_eye_debug_overlay_visible() {
            stop_camera_worker();
        }
        if !is_dev_mode() && !is_calibration_active() {
            if let Ok(mut guard) = EYE_DEBUG_STATE.lock() {
                *guard = None;
            }
        }
        if let Ok(mut lock) = GLOBAL_GAZE_POS.lock() {
            *lock = None;
        }
    }
}

static PREVIEW_TEXTURE_CACHE: Mutex<Option<(Instant, egui::TextureHandle)>> = Mutex::new(None);

/// Dessine la vignette vidéo avec les boîtes vertes et réticules oculaires dans l'interface egui
pub fn render_eye_preview_widget(ui: &mut egui::Ui, desired_width: f32) {
    ensure_camera_worker_running();
    ui.ctx().request_repaint_after(Duration::from_millis(33));

    let source = get_eye_tracker_source();
    if source != EyeTrackerSource::WebcamNeural {
        // HUD visuel pour les trackers matériels (Tobii, GazePoint, OpenTrack)
        let (rect, _) = ui.allocate_exact_size(egui::vec2(desired_width, 110.0), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, egui::CornerRadius::same(8), egui::Color32::from_rgb(14, 18, 24));

        let is_connected = is_eye_hardware_connected();
        let stroke_color = if is_connected { egui::Color32::from_rgb(0, 230, 118) } else { egui::Color32::from_rgb(255, 140, 0) };
        painter.rect_stroke(rect, egui::CornerRadius::same(8), egui::Stroke::new(1.5, stroke_color), egui::StrokeKind::Outside);

        let center = rect.center();
        let title = match source {
            EyeTrackerSource::OpenGazeTcp => "🎯 Hardware Eye Tracker (OpenGaze / Tobii / GazePoint)",
            EyeTrackerSource::OpenTrackUdp => "📡 Hardware Eye Tracker (OpenTrack / FreeTrack)",
            _ => "",
        };
        painter.text(egui::pos2(center.x, rect.min.y + 16.0), egui::Align2::CENTER_CENTER, title, egui::FontId::proportional(12.0), egui::Color32::WHITE);

        // Cible réticule
        let raw_gaze = get_latest_raw_gaze().unwrap_or_default();
        let reticle_x = center.x + raw_gaze.eye_x * (rect.width() * 0.40);
        let reticle_y = center.y + 10.0 + raw_gaze.eye_y * (rect.height() * 0.28);
        let target_pos = egui::pos2(reticle_x, reticle_y);

        painter.circle_stroke(target_pos, 14.0, egui::Stroke::new(1.5, stroke_color));
        painter.circle_filled(target_pos, 3.5, stroke_color);
        painter.line_segment([egui::pos2(target_pos.x - 18.0, target_pos.y), egui::pos2(target_pos.x + 18.0, target_pos.y)], egui::Stroke::new(1.0, stroke_color));
        painter.line_segment([egui::pos2(target_pos.x, target_pos.y - 18.0), egui::pos2(target_pos.x, target_pos.y + 18.0)], egui::Stroke::new(1.0, stroke_color));

        let hz = get_eye_hardware_hz();
        let status_desc = if is_connected {
            format!("Connecté (Fréquence : {} Hz) — Gaze: ({:+.2}, {:+.2})", hz, raw_gaze.eye_x, raw_gaze.eye_y)
        } else {
            let addr = get_eye_hardware_addr();
            format!("En attente du flux matériel sur {}...", addr)
        };
        painter.text(
            egui::pos2(center.x, rect.max.y - 14.0),
            egui::Align2::CENTER_CENTER,
            status_desc,
            egui::FontId::proportional(10.5),
            if is_connected { egui::Color32::from_rgb(0, 230, 118) } else { egui::Color32::from_rgb(255, 180, 50) },
        );
        return;
    }

    if let Some(debug_state) = get_eye_debug_state() {
        if debug_state.width > 0 && debug_state.height > 0 && !debug_state.preview_crop.is_empty() {
            let texture = {
                let mut cache_guard = PREVIEW_TEXTURE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
                let need_upload = cache_guard.as_ref().map_or(true, |(ts, _)| *ts != debug_state.timestamp);
                if need_upload {
                    let color_image = egui::ColorImage::from_rgba_unmultiplied(
                        [debug_state.width, debug_state.height],
                        &debug_state.preview_crop,
                    );
                    let handle = ui.ctx().load_texture(
                        "eyetracking_camera_preview",
                        color_image,
                        egui::TextureOptions::LINEAR,
                    );
                    *cache_guard = Some((debug_state.timestamp, handle.clone()));
                    handle
                } else {
                    cache_guard.as_ref().unwrap().1.clone()
                }
            };

            let aspect = debug_state.height as f32 / debug_state.width.max(1) as f32;
            let max_w = ui.available_width().max(80.0);
            let display_w = desired_width.min(max_w).max(120.0);
            let display_h = (display_w * aspect).round();

            let (rect, _resp) = ui.allocate_exact_size(egui::vec2(display_w, display_h), egui::Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, egui::CornerRadius::same(6), egui::Color32::from_rgb(18, 20, 24));
            painter.image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            painter.rect_stroke(
                rect,
                egui::CornerRadius::same(6),
                egui::Stroke::new(1.5, egui::Color32::from_rgb(0, 230, 118)),
                egui::StrokeKind::Outside,
            );
            return;
        }
    }

    let max_w = ui.available_width().max(80.0);
    let display_w = desired_width.min(max_w).max(120.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(display_w, 90.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, egui::CornerRadius::same(6), egui::Color32::from_rgb(20, 22, 28));
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(6),
        egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
        egui::StrokeKind::Outside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "📷 Initialisation du flux vidéo...",
        egui::FontId::proportional(11.5),
        egui::Color32::from_gray(160),
    );
}

/// Affiche la vignette flottante de retour de caméra sur l'écran
pub fn show_eye_debug_floating_window(ctx: &egui::Context, theme: &ThemeColors) {
    if !should_show_eye_debug() {
        return;
    }

    let vp_id = egui::ViewportId::from_hash_of("eye_tracking_debug_hud_viewport");
    let builder = egui::ViewportBuilder::default()
        .with_title("👁️ Suivi Oculaire — Retour Caméra")
        .with_transparent(true)
        .with_decorations(false)
        .with_always_on_top()
        .with_inner_size([340.0, 220.0])
        .with_min_inner_size([240.0, 140.0])
        .with_resizable(true);

    ctx.show_viewport_immediate(vp_id, builder, |vp_ctx, _class| {
        vp_ctx.request_repaint_after(Duration::from_millis(33));
        let frame = egui::Frame::new()
            .fill(theme.modal_bg)
            .stroke(egui::Stroke::new(1.2, theme.border))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(8, 6));

        egui::CentralPanel::default().frame(frame).show(vp_ctx, |ui| {
            let cam_name = get_camera_device_name();
            let title = if cam_name.is_empty() {
                "👁️ Retour Caméra (Mode Simulation)".to_string()
            } else {
                format!("👁️ Retour Caméra ({})", cam_name)
            };

            let header = ui.horizontal(|ui| {
                ui.label(egui::RichText::new(title).strong().size(11.5).color(theme.accent));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").on_hover_text("Fermer l'encadré debug").clicked() {
                        set_show_eye_debug_overlay(false);
                    }
                });
            });

            if header.response.dragged() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }

            ui.add_space(2.0);
            let avail_w = ui.available_width();
            render_eye_preview_widget(ui, avail_w);

            ui.add_space(3.0);
            ui.horizontal(|ui| {
                let state_opt = get_eye_debug_state();
                let left_ok = state_opt.as_ref().map_or(false, |s| s.left_pupil.is_some());
                let right_ok = state_opt.as_ref().map_or(false, |s| s.right_pupil.is_some());

                let l_col = if left_ok { egui::Color32::from_rgb(0, 230, 118) } else { egui::Color32::from_rgb(255, 120, 120) };
                let r_col = if right_ok { egui::Color32::from_rgb(0, 230, 118) } else { egui::Color32::from_rgb(255, 120, 120) };

                ui.label(egui::RichText::new(format!("Œil G: {}", if left_ok { "✓" } else { "✗" })).size(10.0).color(l_col));
                ui.label(egui::RichText::new(format!("Œil D: {}", if right_ok { "✓" } else { "✗" })).size(10.0).color(r_col));
                let onnx_ready = is_eyetracking_model_installed();
                let onnx_col = if onnx_ready { egui::Color32::from_rgb(0, 230, 118) } else { egui::Color32::from_rgb(255, 180, 50) };
                ui.label(
                    egui::RichText::new(format!("Moteur: {}", if onnx_ready { "L2CS ONNX" } else { "En attente" }))
                        .size(10.0)
                        .color(onnx_col),
                );
                let off_x = get_eye_offset_x();
                let off_y = get_eye_offset_y();
                ui.label(
                    egui::RichText::new(format!("Off X: {:+.0}px", off_x))
                        .size(10.0)
                        .color(egui::Color32::from_rgb(100, 200, 255)),
                );
                if ui.small_button("◀").on_hover_text("Décaler X gauche (-30px)").clicked() {
                    set_eye_offset_x(off_x - 30.0);
                }
                if ui.small_button("▶").on_hover_text("Décaler X droite (+30px)").clicked() {
                    set_eye_offset_x(off_x + 30.0);
                }
                ui.label(
                    egui::RichText::new(format!("Off Y: {:+.0}px", off_y))
                        .size(10.0)
                        .color(egui::Color32::from_rgb(255, 150, 200)),
                );
                if ui.small_button("▲").on_hover_text("Monter curseur Y (-30px)").clicked() {
                    set_eye_offset_y(off_y - 30.0);
                }
                if ui.small_button("▼").on_hover_text("Descendre curseur Y (+30px)").clicked() {
                    set_eye_offset_y(off_y + 30.0);
                }
                ui.label(
                    egui::RichText::new(format!("Pitch Off: {:+.0}°", get_camera_pitch_offset()))
                        .size(10.0)
                        .color(egui::Color32::from_rgb(255, 180, 100)),
                );
                if ui.small_button("Tare").on_hover_text("Calibrer le pitch neutre sur votre regard actuel").clicked() {
                    tare_camera_pitch_to_current_gaze();
                }
                if ui.small_button("▲").on_hover_text("Monter le pitch (+2°)").clicked() {
                    set_camera_pitch_offset(get_camera_pitch_offset() + 2.0);
                }
                if ui.small_button("▼").on_hover_text("Descendre le pitch (-2°)").clicked() {
                    set_camera_pitch_offset(get_camera_pitch_offset() - 2.0);
                }
                let mut inv = is_eye_invert_x();
                if ui.checkbox(&mut inv, egui::RichText::new("Inv X").size(10.0)).on_hover_text("Inverser l'axe horizontal X").changed() {
                    set_eye_invert_x(inv);
                }
                let mut inv_y = is_eye_invert_y();
                if ui.checkbox(&mut inv_y, egui::RichText::new("Inv Y").size(10.0)).on_hover_text("Inverser l'axe vertical Y").changed() {
                    set_eye_invert_y(inv_y);
                }
                let mut cont = is_eye_continuous_mode();
                if ui.checkbox(&mut cont, egui::RichText::new("Continu").size(10.0)).on_hover_text("Mode joystick relatif : défilement linéaire selon l'angle relatif du regard sans avoir à suivre le curseur").changed() {
                    set_eye_continuous_mode(cont);
                }

                if let Some(m) = get_latest_raw_gaze() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("Gaze: ({:+.2}, {:+.2})", m.eye_x, m.eye_y))
                                .size(10.0)
                                .color(theme.text_user),
                        );
                    });
                }
            });
        });
    });
}

pub fn is_camera_worker_running() -> bool {
    CAMERA_WORKER_RUNNING.load(Ordering::SeqCst)
}

pub fn is_camera_active() -> bool {
    (is_eye_tracking_enabled() || is_calibration_active() || is_dev_mode()) && is_camera_worker_running()
}

/// Active ou désactive le suivi caméra de façon idempotente (sans redémarrer inutilement)
pub fn set_camera_active(active: bool) {
    if active == is_eye_tracking_enabled() {
        return;
    }
    set_eye_tracking_enabled(active);
}

pub fn get_camera_device_name() -> String {
    CAMERA_DEVICE_NAME
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

pub fn set_camera_device_name(name: impl Into<String>) {
    if let Ok(mut guard) = CAMERA_DEVICE_NAME.lock() {
        *guard = name.into();
    }
}

pub fn ensure_camera_worker_running() {
    if !is_camera_worker_running() {
        start_camera_worker();
    }
}

pub fn stop_camera_worker() {
    CAMERA_WORKER_RUNNING.store(false, Ordering::SeqCst);
}

pub fn get_latest_raw_gaze() -> Option<RawGazeMeasurement> {
    LATEST_RAW_GAZE.lock().ok().and_then(|guard| *guard)
}

pub fn set_latest_raw_gaze(measurement: RawGazeMeasurement) {
    if let Ok(mut guard) = LATEST_RAW_GAZE.lock() {
        *guard = Some(measurement);
    }
}

// ============================================================================
// 0. Téléchargement et Gestion du Modèle Neuronal (TFLite / ONNX ~2.6 Mo)
// ============================================================================

pub const EYETRACKING_TFLITE_FILENAME: &str = "iris_landmark.tflite";
pub const EYETRACKING_ONNX_FILENAME: &str = "L2CSNet_gaze.onnx";

/// Miroirs publics de référence pour les poids ONNX de L2CS-Net (Estimation 3D du regard)
pub const DEFAULT_EYETRACKING_MODEL_URLS: &[&str] = &[
    "https://raw.githubusercontent.com/anonympins/libertide/refs/heads/main/models/L2CSNet_gaze.onnx",
    "https://huggingface.co/qualcomm/L2CS-Net/resolve/main/L2CSNet.onnx",
    "https://huggingface.co/spaces/Ankush98/L2CS-Net-Gaze-Estimation/resolve/main/models/L2CSNet_gaze.onnx",
];

pub fn get_eyetracking_model_path() -> PathBuf {
    if let Ok(p) = std::env::var("EYETRACKING_MODEL_PATH") {
        let pb = PathBuf::from(p.trim());
        if !p.trim().is_empty() {
            return pb;
        }
    }
    if let Some(p) = read_env_var_from_file(".env", "EYETRACKING_MODEL_PATH") {
        let pb = PathBuf::from(p.trim());
        if !p.trim().is_empty() {
            return pb;
        }
    }

    // 1. Vérification dans le dossier direct models/
    let direct_models = PathBuf::from("models").join(EYETRACKING_ONNX_FILENAME);
    if direct_models.is_file() {
        return direct_models;
    }
    let direct_models_l2cs = PathBuf::from("models").join("L2CSNet.onnx");
    if direct_models_l2cs.is_file() {
        return direct_models_l2cs;
    }
    let base_dir = PathBuf::from("models").join("eyetracking");
    let onnx_direct = base_dir.join(EYETRACKING_ONNX_FILENAME);
    if onnx_direct.is_file() {
        return onnx_direct;
    }
    let onnx_l2cs = base_dir.join("L2CSNet.onnx");
    if onnx_l2cs.is_file() {
        return onnx_l2cs;
    }
    let onnx_alt = base_dir.join("iris_landmark.onnx");
    if onnx_alt.is_file() {
        return onnx_alt;
    }
    let tflite = base_dir.join(EYETRACKING_TFLITE_FILENAME);
    if tflite.is_file() {
        return tflite;
    }
    // 3. Vérification à la racine du projet
    let root_onnx = PathBuf::from(EYETRACKING_ONNX_FILENAME);
    if root_onnx.is_file() {
        return root_onnx;
    }
    if direct_models.exists() {
        return direct_models;
    }
    onnx_direct
}

pub fn is_eyetracking_model_installed() -> bool {
    let path = get_eyetracking_model_path();
    path.is_file() && path.metadata().map_or(0, |m| m.len()) > 1_000
}

pub fn delete_eyetracking_model() -> Result<(), String> {
    let base_dir = PathBuf::from("models").join("eyetracking");
    let tflite = base_dir.join(EYETRACKING_TFLITE_FILENAME);
    let onnx = base_dir.join(EYETRACKING_ONNX_FILENAME);
    let onnx_l2cs = base_dir.join("L2CSNet.onnx");
    let onnx_alt = base_dir.join("iris_landmark.onnx");
    let custom = get_eyetracking_model_path();

    let mut deleted = false;
    if tflite.is_file() {
        let _ = std::fs::remove_file(&tflite);
        deleted = true;
    }
    if onnx.is_file() {
        let _ = std::fs::remove_file(&onnx);
        deleted = true;
    }
    if onnx_l2cs.is_file() {
        let _ = std::fs::remove_file(&onnx_l2cs);
        deleted = true;
    }
    if onnx_alt.is_file() {
        let _ = std::fs::remove_file(&onnx_alt);
        deleted = true;
    }
    if custom.is_file() && custom != tflite && custom != onnx && custom != onnx_alt {
        let _ = std::fs::remove_file(&custom);
        deleted = true;
    }

    if deleted {
        NEURAL_MODEL_LOADED.store(false, Ordering::Relaxed);
        Ok(())
    } else {
        Err("Fichier modèle introuvable".to_string())
    }
}

pub fn ensure_eyetracking_model_downloaded(event_tx: &Sender<AgentEvent>) -> Result<PathBuf, String> {
    if is_eyetracking_model_installed() {
        NEURAL_MODEL_LOADED.store(true, Ordering::Relaxed);
        return Ok(get_eyetracking_model_path());
    }

    let mut candidate_urls: Vec<String> = Vec::new();
    if let Ok(u) = std::env::var("EYETRACKING_MODEL_URL") {
        candidate_urls.push(u.trim().to_string());
    } else if let Some(u) = read_env_var_from_file(".env", "EYETRACKING_MODEL_URL") {
        candidate_urls.push(u.trim().to_string());
    } else {
        candidate_urls.extend(DEFAULT_EYETRACKING_MODEL_URLS.iter().map(|&s| s.to_string()));
    }

    let target_dir = PathBuf::from("models").join("eyetracking");
    let _ = std::fs::create_dir_all(&target_dir);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Runtime Tokio: {e}"))?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("Client HTTP: {e}"))?;

    let event_tx_clone = event_tx.clone();
    let mut last_error = String::new();
    let mut downloaded_path = None;

    for url in &candidate_urls {
        // Normalisation automatique des URLs GitHub blob vers raw
        let mut download_url = url.trim().to_string();
        if download_url.contains("github.com") && download_url.contains("/blob/") {
            download_url = download_url
                .replace("github.com", "raw.githubusercontent.com")
                .replace("/blob/", "/");
        }

        let clean_url = download_url.split('?').next().unwrap_or(&download_url);
        let is_onnx = clean_url.ends_with(".onnx") || download_url.contains(".onnx");
        let filename = if is_onnx {
            EYETRACKING_ONNX_FILENAME
        } else {
            EYETRACKING_TFLITE_FILENAME
        };
        let model_path = target_dir.join(filename);
        let temp_path = target_dir.join(format!("{}.tmp", filename));

        println!("[Eye-Tracking] Tentative de téléchargement depuis : {download_url} ...");
        let temp_path_thread = temp_path.clone();
        let event_tx_iter = event_tx_clone.clone();
        let req_url = download_url.clone();

        let res = rt.block_on(async {
            use tokio::io::AsyncWriteExt;
            let mut res = client.get(&req_url).send().await.map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                return Err(format!("Statut HTTP {}", res.status()));
            }

            let total_bytes = res.content_length().unwrap_or(2_612_416);
            let mut file = tokio::fs::File::create(&temp_path_thread).await.map_err(|e| e.to_string())?;
            let mut count: u64 = 0;
            let start_time = Instant::now();
            let mut last_update = Instant::now();

            while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
                file.write_all(&chunk).await.map_err(|e| e.to_string())?;
                count += chunk.len() as u64;

                if last_update.elapsed() >= Duration::from_millis(80) || count == total_bytes {
                    last_update = Instant::now();
                    let total_mb = total_bytes as f32 / (1024.0 * 1024.0);
                    let downloaded_mb = count as f32 / (1024.0 * 1024.0);
                    let elapsed = start_time.elapsed().as_secs_f32().max(0.001);
                    let speed_mb_s = downloaded_mb / elapsed;
                    let percent = (count as f32 / total_bytes as f32) * 100.0;

                    let _ = event_tx_iter.send(AgentEvent::DownloadProgress {
                        label: format!("Modèle Eye-Tracking ({filename})"),
                        percent,
                        downloaded_mb,
                        total_mb,
                        speed_mb_s,
                    });
                }
            }
            file.flush().await.map_err(|e| e.to_string())?;
            if count < 1_000 {
                return Err(format!("Fichier téléchargé trop petit ({count} octets)"));
            }
            Ok::<(), String>(())
        });

        match res {
            Ok(()) => {
                if std::fs::rename(&temp_path, &model_path).is_ok() {
                    println!("[Eye-Tracking] Modèle prêt dans {}", model_path.display());
                    downloaded_path = Some(model_path);
                    break;
                }
            }
            Err(e) => {
                last_error = e;
                let _ = std::fs::remove_file(&temp_path);
            }
        }
    }

    if let Some(path) = downloaded_path {
        NEURAL_MODEL_LOADED.store(true, Ordering::Relaxed);
        Ok(path)
    } else if is_eyetracking_model_installed() {
        NEURAL_MODEL_LOADED.store(true, Ordering::Relaxed);
        Ok(get_eyetracking_model_path())
    } else {
        Err(format!("Impossible de télécharger le modèle L2CS-Net ({last_error}). Vous pouvez déposer manuellement L2CSNet_gaze.onnx dans models/ ou models/eyetracking/"))
    }
}

// ============================================================================
// 1. Filtre One Euro 2D (Anti-Jitter à latence nulle)
// ============================================================================

#[derive(Clone, Debug)]
pub struct LowPassFilter {
    y: Option<f32>,
}

impl LowPassFilter {
    #[inline]
    pub fn new() -> Self {
        Self { y: None }
    }

    pub fn filter(&mut self, value: f32, alpha: f32) -> f32 {
        let value = if value.is_finite() { value } else { 0.0 };
        let alpha = if alpha.is_finite() { alpha.clamp(0.0, 1.0) } else { 1.0 };
        let prev = match self.y {
            Some(prev_val) if prev_val.is_finite() => prev_val,
            _ => {
                self.y = Some(value);
                return value;
            }
        };
        let filtered = prev + alpha * (value - prev);
        let safe_filtered = if filtered.is_finite() { filtered } else { value };
        self.y = Some(safe_filtered);
        safe_filtered
    }

    #[inline]
    pub fn reset(&mut self) {
        self.y = None;
    }
}

#[derive(Clone, Debug)]
pub struct OneEuroFilter2D {
    freq: f32,
    min_cutoff: f32,
    beta: f32,
    d_cutoff: f32,
    x_filt: LowPassFilter,
    y_filt: LowPassFilter,
    dx_filt: LowPassFilter,
    dy_filt: LowPassFilter,
    last_time: Option<Instant>,
}

impl OneEuroFilter2D {
    pub fn new(min_cutoff: f32, beta: f32) -> Self {
        Self {
            freq: 60.0,
            min_cutoff, // ~0.8 Hz : lissage stable lors des fixations
            beta,       // ~0.015 : vélocité pour supprimer la latence des saccades
            d_cutoff: 1.0,
            x_filt: LowPassFilter::new(),
            y_filt: LowPassFilter::new(),
            dx_filt: LowPassFilter::new(),
            dy_filt: LowPassFilter::new(),
            last_time: None,
        }
    }

    #[inline]
    fn alpha(rate: f32, cutoff: f32) -> f32 {
        let tau = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
        let te = 1.0 / rate.max(1.0);
        1.0 / (1.0 + tau / te)
    }

    pub fn reset(&mut self) {
        self.x_filt.reset();
        self.y_filt.reset();
        self.dx_filt.reset();
        self.dy_filt.reset();
        self.last_time = None;
    }

    pub fn filter(&mut self, (x, y): (f32, f32)) -> (f32, f32) {
        let x = if x.is_finite() { x } else { 0.0 };
        let y = if y.is_finite() { y } else { 0.0 };
        let now = Instant::now();
        if let Some(prev) = self.last_time {
            let dt = now.duration_since(prev).as_secs_f32().max(0.001);
            if dt.is_finite() && dt > 0.0 {
                self.freq = (1.0 / dt).clamp(1.0, 240.0);
            }
        }
        self.last_time = Some(now);

        let dx = self.x_filt.y.map_or(0.0, |prev_x| (x - prev_x) * self.freq);
        let dy = self.y_filt.y.map_or(0.0, |prev_y| (y - prev_y) * self.freq);

        let d_alpha = Self::alpha(self.freq, self.d_cutoff);
        let edx = self.dx_filt.filter(dx, d_alpha);
        let edy = self.dy_filt.filter(dy, d_alpha);

        let speed = edx.hypot(edy);
        let cutoff = self.min_cutoff + self.beta * (if speed.is_finite() { speed } else { 0.0 });
        let alpha = Self::alpha(self.freq, cutoff);

        let fx = self.x_filt.filter(x, alpha);
        let fy = self.y_filt.filter(y, alpha);
        (
            if fx.is_finite() { fx } else { x },
            if fy.is_finite() { fy } else { y },
        )
    }
}

// ============================================================================
// 2. Modèle de Calibration Polynomiale 2D (Moindres Carrés)
// ============================================================================

pub const GAZE_MODEL_TERMS: usize = 6;

#[derive(Clone, Debug)]
pub struct CalibrationSample {
    pub eye_x: f32,
    pub eye_y: f32,
    pub head_yaw: f32,
    pub head_pitch: f32,
    pub screen_x: f32,
    pub screen_y: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GazePolynomialModel {
    pub coeff_x: [f32; GAZE_MODEL_TERMS],
    pub coeff_y: [f32; GAZE_MODEL_TERMS],
    pub rms_error_px: f32,
    pub is_trained: bool,
}

impl Default for GazePolynomialModel {
    fn default() -> Self {
        Self {
            coeff_x: [0.0; GAZE_MODEL_TERMS],
            coeff_y: [0.0; GAZE_MODEL_TERMS],
            rms_error_px: 0.0,
            is_trained: false,
        }
    }
}

impl GazePolynomialModel {
    pub fn calibration_file_path() -> PathBuf {
        PathBuf::from("models").join("eyetracking").join("calibration.json")
    }

    /// Vérifie si le modèle possède une sensibilité/pente suffisante pour ne pas être plat
    pub fn is_valid(&self) -> bool {
        self.is_trained
            && (self.coeff_x[1].abs() > 15.0 || self.coeff_x[3].abs() > 1e-4)
            && (self.coeff_y[2].abs() > 15.0 || self.coeff_y[4].abs() > 1e-4)
            && self.rms_error_px <= 260.0
    }

    pub fn save_to_file(&self) {
        let path = Self::calibration_file_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    pub fn load_from_file() -> Option<Self> {
        let path = Self::calibration_file_path();
        if path.is_file() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(model) = serde_json::from_str::<Self>(&content) {
                    if model.is_valid() {
                        return Some(model);
                    }
                }
            }
        }
        None
    }

    pub fn delete_calibration_file() -> Result<(), String> {
        let path = Self::calibration_file_path();
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    }

    pub fn predict(&self, eye_x: f32, eye_y: f32, head_yaw: f32, head_pitch: f32) -> (f32, f32) {
        if !self.is_valid() {
            return (eye_x, eye_y);
        }
        let x = if eye_x.is_finite() { eye_x } else { 0.0 };
        let y = if eye_y.is_finite() { eye_y } else { 0.0 };
        // Modèle biquadratique 2D canonique (6 termes) : [1, x, y, x^2, y^2, x*y]
        let terms = [
            1.0,
            x,
            y,
            x * x,
            y * y,
            x * y,
        ];

        let px: f32 = terms.iter().zip(&self.coeff_x).map(|(t, c)| t * c).sum();
        let py: f32 = terms.iter().zip(&self.coeff_y).map(|(t, c)| t * c).sum();
        if px.is_finite() && py.is_finite() {
            (px, py)
        } else {
            (eye_x, eye_y)
        }
    }

    pub fn fit(&mut self, samples: &[CalibrationSample]) -> bool {
        const N_TERMS: usize = GAZE_MODEL_TERMS;
        if samples.len() < N_TERMS {
            return false;
        }

        let mut ata = [[0.0f64; N_TERMS]; N_TERMS];
        let mut at_b_x = [0.0f64; N_TERMS];
        let mut at_b_y = [0.0f64; N_TERMS];

        for s in samples {
            let x = if s.eye_x.is_finite() { s.eye_x as f64 } else { 0.0 };
            let y = if s.eye_y.is_finite() { s.eye_y as f64 } else { 0.0 };
            let t = [
                1.0f64,
                x,
                y,
                x * x,
                y * y,
                x * y,
            ];

            let sx = if s.screen_x.is_finite() { s.screen_x as f64 } else { 0.0 };
            let sy = if s.screen_y.is_finite() { s.screen_y as f64 } else { 0.0 };

            for i in 0..N_TERMS {
                at_b_x[i] += t[i] * sx;
                at_b_y[i] += t[i] * sy;
                for j in 0..N_TERMS {
                    ata[i][j] += t[i] * t[j];
                }
            }
        }

        // Régularisation Ridge (lambda = 0.5) garantissant une matrice définie positive et stable
        for i in 0..N_TERMS {
            ata[i][i] += 0.5;
        }

        let sol_x = solve_linear_system::<N_TERMS>(ata, at_b_x);
        let sol_y = solve_linear_system::<N_TERMS>(ata, at_b_y);

        if let (Some(cx), Some(cy)) = (sol_x, sol_y) {
            let mut new_cx = [0.0f32; N_TERMS];
            let mut new_cy = [0.0f32; N_TERMS];
            for i in 0..N_TERMS {
                if !cx[i].is_finite() || !cy[i].is_finite() {
                    return false;
                }
                new_cx[i] = cx[i] as f32;
                new_cy[i] = cy[i] as f32;
            }
            self.coeff_x = new_cx;
            self.coeff_y = new_cy;
            self.is_trained = true;

            // Calcul de l'erreur résiduelle quadratique moyenne (RMS) en pixels
            let mut sum_sq_err = 0.0f32;
            for s in samples {
                let (pred_x, pred_y) = self.predict(s.eye_x, s.eye_y, s.head_yaw, s.head_pitch);
                let sq = (pred_x - s.screen_x).powi(2) + (pred_y - s.screen_y).powi(2);
                sum_sq_err += sq;
            }
            self.rms_error_px = (sum_sq_err / samples.len().max(1) as f32).sqrt();

            // Rejet automatique de tout modèle dont le RMS dépasse 260 px
            if self.rms_error_px > 260.0 || !self.rms_error_px.is_finite() {
                println!(
                    "[Eye-Tracking Calibration] Modèle rejeté : Erreur RMS ({:.1} px) > 260 px",
                    self.rms_error_px
                );
                self.is_trained = false;
                self.coeff_x = [0.0; N_TERMS];
                self.coeff_y = [0.0; N_TERMS];
                return false;
            }

            self.save_to_file();
            true
        } else {
            false
        }
    }
}

fn solve_linear_system<const N: usize>(mut a: [[f64; N]; N], mut b: [f64; N]) -> Option<[f64; N]> {
    for i in 0..N {
        let mut pivot = i;
        // Recherche du pivot partiel maximum
        for k in (i + 1)..N {
            if a[k][i].abs() > a[pivot][i].abs() {
                pivot = k;
            }
        }
        if a[pivot][i].abs() < 1e-12 {
            return None;
        }
        if pivot != i {
            a.swap(i, pivot);
            b.swap(i, pivot);
        }

        let denom = a[i][i];
        for j in i..N {
            a[i][j] /= denom;
        }
        b[i] /= denom;

        for k in 0..N {
            if k != i {
                let factor = a[k][i];
                for j in i..N {
                    a[k][j] -= factor * a[i][j];
                }
                b[k] -= factor * b[i];
            }
        }
    }
    Some(b)
}

// ============================================================================
// 3. Session de Calibration 9 Points
// ============================================================================

pub const CALIBRATION_POINTS_9: [(f32, f32); 9] = [
    (0.08, 0.08), (0.50, 0.08), (0.92, 0.08),
    (0.08, 0.50), (0.50, 0.50), (0.92, 0.50),
    (0.08, 0.92), (0.50, 0.92), (0.92, 0.92),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CalibrationMode {
    /// Poursuite oculaire continue sur courbe harmonique de Lissajous (Recommandé)
    SmoothPursuitCurve,
    /// Grille discrète classique 9 points
    Grid9Points,
}

/// Calcule les coordonnées normalisées [0.0, 1.0] sur la courbe de Lissajous harmonique
fn lissajous_normalized_target(t_sec: f32, total_sec: f32) -> (f32, f32) {
    let tau = (t_sec / total_sec.max(0.001)).clamp(0.0, 1.0);
    let freq_x = 2.0;
    let freq_y = 3.0;
    let phi_x = std::f32::consts::PI * 0.5;

    // Amplitude de 0.38 centré sur 0.5 -> balayage de 0.12 à 0.88 sans buter dans les bordures
    let nx = 0.5 + 0.38 * (2.0 * std::f32::consts::PI * freq_x * tau + phi_x).sin();
    let ny = 0.5 + 0.38 * (2.0 * std::f32::consts::PI * freq_y * tau).sin();
    (nx, ny)
}

#[derive(Clone, Debug)]
pub struct CalibrationSession {
    pub mode: CalibrationMode,
    pub session_start: Instant,
    pub total_duration: Duration,
    pub current_point_index: usize,
    pub point_dwell_start: Instant,
    pub point_duration: Duration,
    pub accumulated_samples: Vec<CalibrationSample>,
    pub is_active: bool,
}

impl Default for CalibrationSession {
    fn default() -> Self {
        Self {
            mode: CalibrationMode::Grid9Points,
            session_start: Instant::now(),
            total_duration: Duration::from_millis(12000), // 12 secondes de poursuite continue
            current_point_index: 0,
            point_dwell_start: Instant::now(),
            point_duration: Duration::from_millis(1200),
            accumulated_samples: Vec::new(),
            is_active: false,
        }
    }
}

impl CalibrationSession {
    pub fn start(&mut self) {
        set_calibration_active(true);
        set_eye_tracking_enabled(true);
        self.session_start = Instant::now();
        self.current_point_index = 0;
        self.point_dwell_start = Instant::now();
        self.accumulated_samples.clear();
        self.is_active = true;
    }

    pub fn current_screen_target(&self, screen_w: f32, screen_h: f32) -> Option<(f32, f32)> {
        if !self.is_active {
            return None;
        }
        match self.mode {
            CalibrationMode::SmoothPursuitCurve => {
                let elapsed = self.session_start.elapsed().as_secs_f32();
                let total = self.total_duration.as_secs_f32();
                let (nx, ny) = lissajous_normalized_target(elapsed, total);
                Some((nx * screen_w, ny * screen_h))
            }
            CalibrationMode::Grid9Points => {
                if self.current_point_index >= CALIBRATION_POINTS_9.len() {
                    return None;
                }
                let (nx, ny) = CALIBRATION_POINTS_9[self.current_point_index];
                Some((nx * screen_w, ny * screen_h))
            }
        }
    }

    pub fn progress(&self) -> f32 {
        if !self.is_active {
            return 0.0;
        }
        match self.mode {
            CalibrationMode::SmoothPursuitCurve => {
                let elapsed = self.session_start.elapsed().as_secs_f32();
                let total = self.total_duration.as_secs_f32();
                (elapsed / total).clamp(0.0, 1.0)
            }
            CalibrationMode::Grid9Points => {
                let total_points = CALIBRATION_POINTS_9.len() as f32;
                let current = self.current_point_index as f32;
                let point_prog = (self.point_dwell_start.elapsed().as_secs_f32() / self.point_duration.as_secs_f32()).clamp(0.0, 1.0);
                ((current + point_prog) / total_points).clamp(0.0, 1.0)
            }
        }
    }

    pub fn feed_sample_and_advance(
        &mut self,
        eye_x: f32,
        eye_y: f32,
        head_yaw: f32,
        head_pitch: f32,
        screen_w: f32,
        screen_h: f32,
        model: &mut GazePolynomialModel,
    ) -> bool {
        if !self.is_active {
            return false;
        }

        match self.mode {
            CalibrationMode::SmoothPursuitCurve => {
                let elapsed = self.session_start.elapsed();

                // Ignore les 400 premières ms (accrochage fovéal initial)
                if elapsed >= Duration::from_millis(400) {
                    // Compensation de la latence oculomotrice de poursuite (~110 ms)
                    let compensated_time = elapsed.saturating_sub(Duration::from_millis(110));
                    let (nx, ny) = lissajous_normalized_target(
                        compensated_time.as_secs_f32(),
                        self.total_duration.as_secs_f32(),
                    );
                    self.accumulated_samples.push(CalibrationSample {
                        eye_x,
                        eye_y,
                        head_yaw,
                        head_pitch,
                        screen_x: nx * screen_w,
                        screen_y: ny * screen_h,
                    });
                }

                if elapsed >= self.total_duration {
                    self.is_active = false;
                    set_calibration_active(false);
                    model.fit(&self.accumulated_samples);
                    return true;
                }
                false
            }
            CalibrationMode::Grid9Points => {
                if self.current_point_index >= CALIBRATION_POINTS_9.len() {
                    return false;
                }
                let (tx, ty) = self.current_screen_target(screen_w, screen_h).unwrap();
                if self.point_dwell_start.elapsed() >= Duration::from_millis(250) {
                    self.accumulated_samples.push(CalibrationSample {
                        eye_x,
                        eye_y,
                        head_yaw,
                        head_pitch,
                        screen_x: tx,
                        screen_y: ty,
                    });
                }
                if self.point_dwell_start.elapsed() >= self.point_duration {
                    self.current_point_index += 1;
                    self.point_dwell_start = Instant::now();
                    if self.current_point_index < CALIBRATION_POINTS_9.len() {
                        crate::overlay::play_earcon(crate::overlay::Earcon::Navigation);
                    }
                    if self.current_point_index >= CALIBRATION_POINTS_9.len() {
                        self.is_active = false;
                        set_calibration_active(false);
                        model.fit(&self.accumulated_samples);
                        return true;
                    }
                }
                false
            }
        }
    }
}

// ============================================================================
// 4. Semantic Gaze Snapping (Aimant sémantique sur contrôles)
// ============================================================================

/// Attire magnétiquement le point de regard vers le contrôle le plus proche avec hystérésis temporelle
pub fn snap_gaze_to_target_sticky<'a>(
    raw_gaze: (f32, f32),
    targets: &'a [NumberedElementTarget],
    snap_radius: f32, // Recommandé : 95.0 pixels
    last_target_id: Option<usize>,
    last_snap_time: Option<Instant>,
) -> (f32, f32, Option<&'a NumberedElementTarget>) {
    let (gx, gy) = raw_gaze;
    if targets.is_empty() {
        return (gx, gy, None);
    }

    let mut best_target: Option<&'a NumberedElementTarget> = None;
    let mut min_distance = snap_radius;
    let mut current_target_dist: Option<f32> = None;

    // Marge d'arrachage hystérétique pour éviter le sautillement entre boutons proches
    let hysteresis_margin = snap_radius * 0.45;
    let sticky_duration = Duration::from_millis(220);

    for target in targets {
        let cx = (target.rect[0] + target.rect[2]) as f32 / 2.0;
        let cy = (target.rect[1] + target.rect[3]) as f32 / 2.0;

        // Adhérence automatique et prioritaire à l'intérieur de la boîte englobante
        let inside = gx >= target.rect[0] as f32
            && gx <= target.rect[2] as f32
            && gy >= target.rect[1] as f32
            && gy <= target.rect[3] as f32;
        if inside {
            return (cx, cy, Some(target));
        }

        let dx = cx - gx;
        let dy = cy - gy;
        let dist = dx.hypot(dy);

        if Some(target.index) == last_target_id {
            current_target_dist = Some(dist);
        }

        // Favorise la cible active si elle est encore dans la zone étendue
        let effective_dist = if Some(target.index) == last_target_id { dist - hysteresis_margin } else { dist };
        if effective_dist < min_distance {
            min_distance = effective_dist;
            best_target = Some(target);
        }
    }

    if let Some(target) = best_target {
        let cx = (target.rect[0] + target.rect[2]) as f32 / 2.0;
        let cy = (target.rect[1] + target.rect[3]) as f32 / 2.0;
        let real_dist = (cx - gx).hypot(cy - gy);

        // Préservation de l'ancrage sticky pendant la période de transition minimale
        if let (Some(cur_id), Some(t_start), Some(d)) = (last_target_id, last_snap_time, current_target_dist) {
            if cur_id != target.index && t_start.elapsed() < sticky_duration && d < (snap_radius * 1.4) {
                if let Some(old_target) = targets.iter().find(|t| t.index == cur_id) {
                    let ocx = (old_target.rect[0] + old_target.rect[2]) as f32 / 2.0;
                    let ocy = (old_target.rect[1] + old_target.rect[3]) as f32 / 2.0;
                    let norm_d = (d / (snap_radius * 1.4)).clamp(0.0, 1.0);
                    let w = (1.0 - norm_d.powf(1.35)).clamp(0.0, 1.0);
                    return (gx * (1.0 - w) + ocx * w, gy * (1.0 - w) + ocy * w, Some(old_target));
                }
            }
        }

        // Puits de gravité accentué à gradient large en 1 - r^1.35 (aspiration énergique)
        let norm_dist = (real_dist / snap_radius).clamp(0.0, 1.0);
        let weight = (1.0 - norm_dist.powf(1.35)).clamp(0.0, 1.0);
        let snapped_x = gx * (1.0 - weight) + cx * weight;
        let snapped_y = gy * (1.0 - weight) + cy * weight;
        (snapped_x, snapped_y, Some(target))
    } else {
        (gx, gy, None)
    }
}

pub fn snap_gaze_to_target<'a>(raw_gaze: (f32, f32), targets: &'a [NumberedElementTarget], snap_radius: f32) -> (f32, f32, Option<&'a NumberedElementTarget>) {
    snap_gaze_to_target_sticky(raw_gaze, targets, snap_radius, None, None)
}

// ============================================================================
// 5. Contrôleur Principal EyeTrackingController
// ============================================================================

pub struct EyeTrackingController {
    pub filter: OneEuroFilter2D,
    pub model: GazePolynomialModel,
    pub calibration: CalibrationSession,
    pub last_raw_gaze: Option<(f32, f32)>,
    pub raw_history: [(f32, f32); 4],
    pub raw_history_idx: usize,
    pub raw_history_len: usize,
    pub last_snapped_target_id: Option<usize>,
    pub last_snap_timestamp: Option<Instant>,
    pub snap_radius: f32,
    pub sensitivity: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub snapping_enabled: bool,
    pub confirmed_pred_gaze: Option<(f32, f32)>,
    pub pending_candidate: Option<(f32, f32)>,
    pub consecutive_move_frames: usize,
    pub current_lerp_pos: Option<(f32, f32)>,
    pub cursor_vel: (f32, f32),
    pub last_lerp_timestamp: Option<Instant>,
    pub last_target_local: Option<(f32, f32)>,
    pub last_eye_applied_cursor: Option<(i32, i32)>,
    pub mouse_override_until: Option<Instant>,
    pub mouse_override_debounce: Duration,
}

impl Default for EyeTrackingController {
    fn default() -> Self {
        let enabled = std::env::var("EYE_TRACKING_ENABLED")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EYE_TRACKING_ENABLED"))
            .map(|s| s.trim().eq_ignore_ascii_case("true") || s.trim() == "1")
            .unwrap_or(false);
        EYE_TRACKING_ENABLED.store(enabled, Ordering::Relaxed);
        if enabled || is_dev_mode() {
            start_camera_worker();
        }

        let snapping_enabled = std::env::var("EYE_SNAPPING_ENABLED")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EYE_SNAPPING_ENABLED"))
            .map(|s| s.trim().eq_ignore_ascii_case("true") || s.trim() == "1")
            .unwrap_or(true);
        EYE_SNAPPING_ENABLED.store(snapping_enabled, Ordering::Relaxed);

        let snap_radius = std::env::var("EYE_SNAP_RADIUS")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EYE_SNAP_RADIUS"))
            .and_then(|s| s.trim().parse::<f32>().ok())
            .unwrap_or(95.0);

        let sens_val = std::env::var("EYE_TRACKING_SENSITIVITY")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EYE_TRACKING_SENSITIVITY"))
            .and_then(|s| s.trim().parse::<f32>().ok())
            .unwrap_or(1.0);
        EYE_TRACKING_SENSITIVITY.store(sens_val.to_bits(), Ordering::Relaxed);

        let offset_x = std::env::var("EYE_OFFSET_X")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EYE_OFFSET_X"))
            .and_then(|s| s.trim().parse::<f32>().ok())
            .unwrap_or(0.0);
        EYE_OFFSET_X.store(offset_x.to_bits(), Ordering::Relaxed);

        let offset_y = std::env::var("EYE_OFFSET_Y")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EYE_OFFSET_Y"))
            .and_then(|s| s.trim().parse::<f32>().ok())
            .unwrap_or(0.0);
        EYE_OFFSET_Y.store(offset_y.to_bits(), Ordering::Relaxed);

        let pitch_offset = std::env::var("EYE_CAMERA_PITCH_OFFSET")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EYE_CAMERA_PITCH_OFFSET"))
            .and_then(|s| s.trim().parse::<f32>().ok())
            .unwrap_or(-16.0);
        EYE_CAMERA_PITCH_OFFSET.store(pitch_offset.to_bits(), Ordering::Relaxed);

        let invert_x = std::env::var("EYE_INVERT_X")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EYE_INVERT_X"))
            .map(|s| s.trim().eq_ignore_ascii_case("true") || s.trim() == "1")
            .unwrap_or(false);
        EYE_INVERT_X.store(invert_x, Ordering::Relaxed);

        let invert_y = std::env::var("EYE_INVERT_Y")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EYE_INVERT_Y"))
            .map(|s| s.trim().eq_ignore_ascii_case("true") || s.trim() == "1")
            .unwrap_or(false);
        EYE_INVERT_Y.store(invert_y, Ordering::Relaxed);

        let continuous = std::env::var("EYE_CONTINUOUS_MODE")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EYE_CONTINUOUS_MODE"))
            .map(|s| s.trim().eq_ignore_ascii_case("true") || s.trim() == "1")
            .unwrap_or(false);
        EYE_CONTINUOUS_MODE.store(continuous, Ordering::Relaxed);

        let threshold_a = std::env::var("EYE_JOYSTICK_THRESHOLD_A")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EYE_JOYSTICK_THRESHOLD_A"))
            .and_then(|s| s.trim().parse::<f32>().ok())
            .unwrap_or(0.025);
        let threshold_a = if threshold_a < 0.005 { 0.025 } else { threshold_a };
        EYE_JOYSTICK_THRESHOLD_A.store(threshold_a.to_bits(), Ordering::Relaxed);

        let source_str = std::env::var("EYE_TRACKER_SOURCE")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EYE_TRACKER_SOURCE"))
            .unwrap_or_else(|| "webcam".to_string());
        let source = match source_str.to_lowercase().as_str() {
            "opengaze" | "tobii" | "gazepoint" => EyeTrackerSource::OpenGazeTcp,
            "opentrack" | "freetrack" => EyeTrackerSource::OpenTrackUdp,
            _ => EyeTrackerSource::WebcamNeural,
        };
        set_eye_tracker_source(source);

        let mouse_override_ms = std::env::var("EYE_MOUSE_DEBOUNCE_MS")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EYE_MOUSE_DEBOUNCE_MS"))
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(2500);

        let model = GazePolynomialModel::load_from_file().unwrap_or_default();

        Self {
            filter: OneEuroFilter2D::new(0.70, 0.035),
            model,
            calibration: CalibrationSession::default(),
            last_raw_gaze: None,
            raw_history: [(0.0, 0.0); 4],
            raw_history_idx: 0,
            raw_history_len: 0,
            last_snapped_target_id: None,
            last_snap_timestamp: None,
            snap_radius,
            sensitivity: sens_val,
            offset_x,
            offset_y,
            snapping_enabled,
            confirmed_pred_gaze: None,
            pending_candidate: None,
            consecutive_move_frames: 0,
            current_lerp_pos: None,
            cursor_vel: (0.0, 0.0),
            last_lerp_timestamp: None,
            last_target_local: None,
            last_eye_applied_cursor: None,
            mouse_override_until: None,
            mouse_override_debounce: Duration::from_millis(mouse_override_ms),
        }
    }
}

impl EyeTrackingController {
    pub fn start_calibration(&mut self) {
        self.calibration.mode = CalibrationMode::Grid9Points;
        self.calibration.start();
        self.filter.reset();
        self.confirmed_pred_gaze = None;
        self.pending_candidate = None;
        self.consecutive_move_frames = 0;
        self.current_lerp_pos = None;
        self.cursor_vel = (0.0, 0.0);
        self.last_lerp_timestamp = None;
        self.raw_history = [(0.0, 0.0); 4];
        self.raw_history_idx = 0;
        self.raw_history_len = 0;
        self.last_target_local = None;
        self.last_eye_applied_cursor = None;
        self.mouse_override_until = None;
    }

    pub fn cancel_calibration(&mut self) {
        self.calibration.is_active = false;
        set_calibration_active(false);
        self.filter.reset();
        self.confirmed_pred_gaze = None;
        self.pending_candidate = None;
        self.consecutive_move_frames = 0;
        self.current_lerp_pos = None;
        self.cursor_vel = (0.0, 0.0);
        self.last_lerp_timestamp = None;
        self.raw_history = [(0.0, 0.0); 4];
        self.raw_history_idx = 0;
        self.raw_history_len = 0;
        self.last_target_local = None;
        self.last_eye_applied_cursor = None;
        self.mouse_override_until = None;
        if let Ok(mut lock) = GLOBAL_GAZE_POS.lock() {
            *lock = None;
        }
    }

    pub fn handle_escape_if_pressed(&mut self) -> bool {
        if is_escape_key_pressed() && (is_eye_tracking_enabled() || self.calibration.is_active || is_calibration_active()) {
            self.cancel_calibration();
            set_eye_tracking_enabled(false);
            set_calibration_active(false);
            crate::overlay::play_earcon(crate::overlay::Earcon::Navigation);
            true
        } else {
            false
        }
    }

    pub fn reset_calibration(&mut self) {
        self.model = GazePolynomialModel::default();
        let _ = GazePolynomialModel::delete_calibration_file();
        self.filter.reset();
        self.confirmed_pred_gaze = None;
        self.pending_candidate = None;
        self.consecutive_move_frames = 0;
        self.current_lerp_pos = None;
        self.cursor_vel = (0.0, 0.0);
        self.last_lerp_timestamp = None;
        self.raw_history = [(0.0, 0.0); 4];
        self.raw_history_idx = 0;
        self.raw_history_len = 0;
        self.last_target_local = None;
        self.last_raw_gaze = None;
        self.last_snapped_target_id = None;
        self.last_snap_timestamp = None;
        if let Ok(mut lock) = GLOBAL_GAZE_POS.lock() {
            *lock = None;
        }
        crate::overlay::play_earcon(crate::overlay::Earcon::Success);
    }

    pub fn recenter_center_gaze(&mut self, screen_w: f32, screen_h: f32) {
        self.recenter_to_point(screen_w * 0.5, screen_h * 0.5, screen_w, screen_h);
    }

    /// Calibre instantanément le modèle neuronal/polynomial sur la position du curseur
    pub fn recenter_to_cursor(&mut self, cursor_x: f32, cursor_y: f32, screen_w: f32, screen_h: f32) {
        self.recenter_to_point(cursor_x, cursor_y, screen_w, screen_h);
    }

    /// Aligne le regard sur un point cible précis de l'écran en mettant à jour le modèle
    pub fn recenter_to_point(&mut self, target_x: f32, target_y: f32, screen_w: f32, screen_h: f32) {
        if let Some(raw) = get_latest_raw_gaze() {
            self.offset_x = get_eye_offset_x();
            self.offset_y = get_eye_offset_y();
            let raw_x = if is_eye_invert_x() { -raw.eye_x } else { raw.eye_x };
            let raw_y = if is_eye_invert_y() { -raw.eye_y } else { raw.eye_y };

            if is_eye_continuous_mode() {
                // Tare automatique du pitch pour annuler la pente naturelle vers l'écran
                let current_pitch_offset = get_camera_pitch_offset();
                let new_pitch_offset = current_pitch_offset - raw.head_pitch;
                set_camera_pitch_offset(new_pitch_offset);

                // Tare du manche joystick sur les coordonnées oculaires brutes actuelles
                set_eye_offset_x(raw_x);
                set_eye_offset_y(0.0);
                self.offset_x = raw_x;
                self.offset_y = 0.0;
                self.cursor_vel = (0.0, 0.0);
                self.current_lerp_pos = Some((target_x, target_y));
                self.confirmed_pred_gaze = Some((target_x, target_y));
                self.last_target_local = Some((target_x, target_y));
                self.filter.reset();
                if let Ok(mut lock) = GLOBAL_GAZE_POS.lock() {
                    *lock = Some((target_x, target_y));
                }
                crate::overlay::play_earcon(crate::overlay::Earcon::Success);
                return;
            }
            let sens = self.sensitivity;
            let raw_yaw = if is_eye_invert_x() { -raw.head_yaw } else { raw.head_yaw };
            let raw_pitch = if is_eye_invert_y() { -raw.head_pitch } else { raw.head_pitch };
            let (raw_pred_x, raw_pred_y) = if self.model.is_trained {
                let (px, py) = self.model.predict(raw_x, raw_y, raw_yaw, raw_pitch);
                (px + self.offset_x, py + self.offset_y)
            } else {
                (
                    (raw_x.clamp(-1.0, 1.0) * 0.5 + 0.5) * screen_w + self.offset_x,
                    (raw_y.clamp(-1.0, 1.0) * 0.5 + 0.5) * screen_h + self.offset_y,
                )
            };
            let delta_x = target_x - raw_pred_x;
            let delta_y = target_y - raw_pred_y;

            if self.model.is_trained {
                self.model.coeff_x[0] += delta_x;
                self.model.coeff_y[0] += delta_y;
                self.model.save_to_file();
            } else {
                let mut cx = [0.0; GAZE_MODEL_TERMS];
                let mut cy = [0.0; GAZE_MODEL_TERMS];
                cx[0] = target_x - (raw_x * screen_w * 0.5 * sens);
                cx[1] = screen_w * 0.5 * sens;
                cy[0] = target_y - (raw_y * screen_h * 0.5 * sens * 1.45);
                cy[2] = screen_h * 0.5 * sens * 1.45;
                self.model.rms_error_px = 35.0;
                self.model.is_trained = true;
                self.model.save_to_file();
            }

            self.filter.reset();
            self.pending_candidate = None;
            self.consecutive_move_frames = 0;
            self.confirmed_pred_gaze = Some((target_x, target_y));
            self.current_lerp_pos = Some((target_x, target_y));
            self.cursor_vel = (0.0, 0.0);
            self.raw_history = [(0.0, 0.0); 4];
            self.raw_history_idx = 0;
            self.raw_history_len = 0;
            self.last_lerp_timestamp = Some(Instant::now());
            self.last_target_local = Some((target_x, target_y));
            if let Ok(mut lock) = GLOBAL_GAZE_POS.lock() {
                *lock = Some((target_x, target_y));
            }
            crate::overlay::play_earcon(crate::overlay::Earcon::Success);
        }
    }

    #[inline]
    pub fn is_mouse_overridden(&self) -> bool {
        if let Some(until) = self.mouse_override_until {
            Instant::now() < until
        } else {
            false
        }
    }

    pub fn trigger_mouse_override(&mut self, current_phys: (i32, i32), work_area_offset: (f32, f32), ppp: f32) {
        let now = Instant::now();
        self.mouse_override_until = Some(now + self.mouse_override_debounce);
        self.last_eye_applied_cursor = Some(current_phys);
        let local_x = (current_phys.0 as f32 / ppp) - work_area_offset.0;
        let local_y = (current_phys.1 as f32 / ppp) - work_area_offset.1;
        self.current_lerp_pos = Some((local_x, local_y));
        self.cursor_vel = (0.0, 0.0);
        self.confirmed_pred_gaze = Some((local_x, local_y));
        self.last_target_local = Some((local_x, local_y));
        self.pending_candidate = None;
    }

    pub fn process_gaze_input(
        &mut self,
        raw_eye_x: f32,
        raw_eye_y: f32,
        head_yaw: f32,
        head_pitch: f32,
        screen_w: f32,
        screen_h: f32,
        targets: &[NumberedElementTarget],
    ) -> (f32, f32, Option<usize>) {
        self.process_gaze_input_scaled(
            raw_eye_x, raw_eye_y, head_yaw, head_pitch, screen_w, screen_h, (0.0, 0.0), 1.0, targets,
        )
    }

    pub fn process_gaze_input_scaled(
        &mut self,
        raw_eye_x: f32,
        raw_eye_y: f32,
        head_yaw: f32,
        head_pitch: f32,
        screen_w: f32,
        screen_h: f32,
        work_area_offset: (f32, f32),
        ppp: f32,
        targets: &[NumberedElementTarget],
    ) -> (f32, f32, Option<usize>) {
        if self.handle_escape_if_pressed() {
            return (screen_w / 2.0, screen_h / 2.0, None);
        }

        if !is_eye_tracking_enabled() && !is_dev_mode() && !self.calibration.is_active && !is_calibration_active() {
            self.confirmed_pred_gaze = None;
            self.pending_candidate = None;
            self.consecutive_move_frames = 0;
            self.current_lerp_pos = None;
            self.cursor_vel = (0.0, 0.0);
            self.last_lerp_timestamp = None;
            self.last_target_local = None;
            if let Ok(mut lock) = GLOBAL_GAZE_POS.lock() {
                *lock = None;
            }
            return (screen_w / 2.0, screen_h / 2.0, None);
        }

        self.sensitivity = get_eye_tracking_sensitivity();
        let effective_raw_x = if is_eye_invert_x() { -raw_eye_x } else { raw_eye_x };
        let effective_raw_y = if is_eye_invert_y() { -raw_eye_y } else { raw_eye_y };
        let effective_yaw = if is_eye_invert_x() { -head_yaw } else { head_yaw };
        let effective_pitch = if is_eye_invert_y() { -head_pitch } else { head_pitch };

        if self.calibration.is_active {
            let finished = self.calibration.feed_sample_and_advance(
                effective_raw_x,
                effective_raw_y,
                effective_yaw,
                effective_pitch,
                screen_w,
                screen_h,
                &mut self.model,
            );
            if finished {
                crate::overlay::play_earcon(crate::overlay::Earcon::Success);
            }
            let target = self.calibration.current_screen_target(screen_w, screen_h).unwrap_or((screen_w / 2.0, screen_h / 2.0));
            if let Ok(mut lock) = GLOBAL_GAZE_POS.lock() {
                *lock = Some((target.0, target.1));
            }
            return (target.0, target.1, None);
        }

        if self.is_mouse_overridden() {
            if let Some((mx, my)) = self.current_lerp_pos {
                if let Ok(mut lock) = GLOBAL_GAZE_POS.lock() {
                    *lock = Some((mx, my));
                }
                return (mx, my, None);
            }
        }

        // Enregistrement dans le buffer circulaire pour détection instantanée de saccade sur 2 frames
        // Le signe de (effective_raw - prev) détermine à lui seul le sens de variation, quelle que soit la position absolue
        let (d2_x, d2_y) = if self.raw_history_len >= 2 {
            let prev_idx = (self.raw_history_idx + 4 - 2) % 4;
            let prev = self.raw_history[prev_idx];
            (effective_raw_x - prev.0, effective_raw_y - prev.1)
        } else {
            (0.0, 0.0)
        };
        self.raw_history[self.raw_history_idx] = (effective_raw_x, effective_raw_y);
        self.raw_history_idx = (self.raw_history_idx + 1) % 4;
        if self.raw_history_len < 4 {
            self.raw_history_len += 1;
        }

        // ====================================================================
        // Traitement du Regard : Mode Joystick Force Vectorielle ou Absolu
        // ====================================================================
        self.offset_x = get_eye_offset_x();
        self.offset_y = get_eye_offset_y();

        let now = Instant::now();
        let dt = self
            .last_lerp_timestamp
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.016)
            .clamp(0.001, 0.050);
        self.last_lerp_timestamp = Some(now);

        if is_eye_continuous_mode() {
            // Déviation par rapport au centre calibré (offset_x, offset_y)
            // effective_raw_x intègre déjà is_eye_invert_x()
            let raw_vec_x = (effective_raw_x - self.offset_x) * 1.60;
            // Gain Y décisif (2.80) : compense l'amplitude angulaire verticale réduite de l'œil (~11.5° vs ~25°)
            let raw_vec_y = (effective_raw_y - self.offset_y) * 2.80;

            // Filtrage One Euro du vecteur de force pour gommer tout bruit de caméra floue
            let (filt_fx, filt_fy) = self.filter.filter((raw_vec_x, raw_vec_y));
            let deadzone_x = get_eye_joystick_threshold_a();
            // Seuil de zone morte en Y réduit de 35% : toute légère inclinaison verticale déclenche immédiatement le curseur
            let deadzone_y = (deadzone_x * 0.65).max(0.004);

            // Distance normalisée par rapport à l'ellipse de zone morte
            let norm_dist = ((filt_fx / deadzone_x).powi(2) + (filt_fy / deadzone_y).powi(2)).sqrt();

            // Calcul de la poussée vectorielle (vitesse cible en px/sec)
            let (target_vx, target_vy) = if norm_dist > 1.0 {
                let excess = (norm_dist - 1.0).clamp(0.0, 1.5);
                let curve = excess.powf(1.15);
                let max_speed = 1850.0 * self.sensitivity;

                let scaled_fy = filt_fy * 1.35;
                let dir_len = filt_fx.hypot(scaled_fy).max(1e-4);
                let dir_x = filt_fx / dir_len;
                let dir_y = scaled_fy / dir_len;
                (dir_x * curve * max_speed, dir_y * curve * max_speed)
            } else {
                (0.0, 0.0)
            };

            // Amortissement et frottement cinétique
            let accel_rate = if target_vx != 0.0 || target_vy != 0.0 { 14.0 } else { 22.0 };
            let blend = (accel_rate * dt).clamp(0.0, 1.0);
            self.cursor_vel.0 += (target_vx - self.cursor_vel.0) * blend;
            self.cursor_vel.1 += (target_vy - self.cursor_vel.1) * blend;

            if self.cursor_vel.0.hypot(self.cursor_vel.1) < 1.0 {
                self.cursor_vel = (0.0, 0.0);
            }

            let (prev_x, prev_y) = self.current_lerp_pos.unwrap_or((screen_w * 0.5, screen_h * 0.5));
            let mut pos_x = (prev_x + self.cursor_vel.0 * dt).clamp(0.0, screen_w);
            let mut pos_y = (prev_y + self.cursor_vel.1 * dt).clamp(0.0, screen_h);

            // Snapping magnétique doux sur les éléments cliquables proches
            let phys_x = (work_area_offset.0 + pos_x) * ppp;
            let phys_y = (work_area_offset.1 + pos_y) * ppp;

            let should_snap = self.snapping_enabled && is_eye_snapping_enabled() && !targets.is_empty();
            let (snapped_phys_x, snapped_phys_y, snapped_target) = if should_snap {
                snap_gaze_to_target_sticky(
                    (phys_x, phys_y),
                    targets,
                    self.snap_radius * ppp,
                    self.last_snapped_target_id,
                    self.last_snap_timestamp,
                )
            } else {
                (phys_x, phys_y, None)
            };

            let target_idx = snapped_target.map(|t| t.index);
            if target_idx != self.last_snapped_target_id {
                self.last_snap_timestamp = Some(Instant::now());
            }
            self.last_snapped_target_id = target_idx;

            if should_snap && snapped_target.is_some() {
                let snap_local_x = (snapped_phys_x / ppp) - work_area_offset.0;
                let snap_local_y = (snapped_phys_y / ppp) - work_area_offset.1;
                pos_x = pos_x * 0.78 + snap_local_x * 0.22;
                pos_y = pos_y * 0.78 + snap_local_y * 0.22;
            }

            self.current_lerp_pos = Some((pos_x, pos_y));
            self.last_target_local = Some((pos_x, pos_y));
            self.confirmed_pred_gaze = Some((pos_x, pos_y));
            self.last_raw_gaze = Some((effective_raw_x, effective_raw_y));

            if let Ok(mut lock) = GLOBAL_GAZE_POS.lock() {
                *lock = Some((pos_x, pos_y));
            }

            return (pos_x, pos_y, target_idx);
        }

        // 1. Calcul de la coordonnée cible brute sur l'écran
        let (raw_screen_x, raw_screen_y) = if self.model.is_valid() {
            let (px, py) = self.model.predict(
                effective_raw_x,
                effective_raw_y,
                effective_yaw,
                effective_pitch,
            );
            (px + self.offset_x, py + self.offset_y)
        } else {
            let center_x = screen_w * 0.5;
            let center_y = screen_h * 0.5;
            (
                center_x + (effective_raw_x * center_x * self.sensitivity) + self.offset_x,
                center_y + (effective_raw_y * center_y * self.sensitivity * 1.45) + self.offset_y,
            )
        };

        let raw_screen_x = raw_screen_x.clamp(0.0, screen_w);
        let raw_screen_y = raw_screen_y.clamp(0.0, screen_h);

        // 2. Filtrage One Euro (stabilisation haute fidélité sans latence sur les saccades)
        let (filt_x, filt_y) = self.filter.filter((raw_screen_x, raw_screen_y));

        let target_local_x = filt_x;
        let target_local_y = filt_y;
        self.confirmed_pred_gaze = Some((filt_x, filt_y));

        self.last_raw_gaze = Some((effective_raw_x, effective_raw_y));

        // 4. Snapping sémantique (aimant sticky sur les contrôles cliquables)
        let phys_x = (work_area_offset.0 + target_local_x) * ppp;
        let phys_y = (work_area_offset.1 + target_local_y) * ppp;

        let should_snap = self.snapping_enabled && is_eye_snapping_enabled() && !targets.is_empty();
        let (snapped_phys_x, snapped_phys_y, snapped_target) = if should_snap {
            snap_gaze_to_target_sticky(
                (phys_x, phys_y),
                targets,
                self.snap_radius * ppp,
                self.last_snapped_target_id,
                self.last_snap_timestamp,
            )
        } else {
            (phys_x, phys_y, None)
        };

        let target_idx = snapped_target.map(|t| t.index);
        if target_idx != self.last_snapped_target_id {
            self.last_snap_timestamp = Some(Instant::now());
        }
        self.last_snapped_target_id = target_idx;

        let effective_target_x = (snapped_phys_x / ppp) - work_area_offset.0;
        let effective_target_y = (snapped_phys_y / ppp) - work_area_offset.1;

        // 5. Simulation physique d'inertie & glisse feutrée (effet boule de billard)
        let now = Instant::now();
        let dt = self
            .last_lerp_timestamp
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.016)
            .clamp(0.001, 0.1);
        self.last_lerp_timestamp = Some(now);

        let (mut cur_x, mut cur_y) = self.current_lerp_pos.unwrap_or((effective_target_x, effective_target_y));
        if !cur_x.is_finite() { cur_x = effective_target_x; }
        if !cur_y.is_finite() { cur_y = effective_target_y; }

        let (mut vel_x, mut vel_y) = self.cursor_vel;
        if !vel_x.is_finite() { vel_x = 0.0; }
        if !vel_y.is_finite() { vel_y = 0.0; }

        // Modèle dynamique d'ordre 2 :
        // omega = réactivité / célérité du déplacement (20 rad/s = temps de stabilisation ~200ms)
        // zeta  = coefficient d'amortissement (0.88 = glisse fluide avec décélération douce feutrée sans oscillation)
        let omega = 20.0f32;
        let zeta = 0.88f32;
        let k = omega * omega;
        let c = 2.0 * zeta * omega;

        // Sous-échantillonnage temporel (pas de 2ms) pour garantir une intégration symplectique sans dérive
        let sub_dt = 0.002f32;
        let steps = (dt / sub_dt).ceil().max(1.0).min(50.0) as usize;
        let step_dt = dt / steps as f32;

        for _ in 0..steps {
            let fx = k * (effective_target_x - cur_x) - c * vel_x;
            let fy = k * (effective_target_y - cur_y) - c * vel_y;

            vel_x += fx * step_dt;
            vel_y += fy * step_dt;

            cur_x += vel_x * step_dt;
            cur_y += vel_y * step_dt;
        }

        // Seuil d'accrochage à l'arrêt complet pour éviter les micro-frémissements sous le pixel
        if (effective_target_x - cur_x).hypot(effective_target_y - cur_y) < 0.8 && vel_x.hypot(vel_y) < 8.0 {
            cur_x = effective_target_x;
            cur_y = effective_target_y;
            vel_x = 0.0;
            vel_y = 0.0;
        }

        let local_x = cur_x.clamp(0.0, screen_w);
        let local_y = cur_y.clamp(0.0, screen_h);

        self.cursor_vel = (vel_x, vel_y);
        self.current_lerp_pos = Some((local_x, local_y));
        self.last_target_local = Some((local_x, local_y));

        if let Ok(mut lock) = GLOBAL_GAZE_POS.lock() {
            *lock = Some((local_x, local_y));
        }

        (local_x, local_y, target_idx)
    }
}

// ============================================================================
// 6. Pipeline L2CS-Net Direct (Inférence ONNX & Décodage Binné Softmax)
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PixelFormat {
    Yuy2,
    Rgb24,
    Rgb32,
    Nv12,
    Grayscale,
}

/// Initialise ou récupère la session ONNX Runtime pour L2CS-Net
fn get_or_init_l2cs_session() -> Result<(), String> {
    let mut guard = L2CS_SESSION.lock().map_err(|e| e.to_string())?;
    if guard.is_some() {
        return Ok(());
    }
    let model_path = get_eyetracking_model_path();
    if !model_path.is_file() {
        return Err(format!("Modèle ONNX introuvable à : {} (vérifiez models/ ou models/eyetracking/)", model_path.display()));
    }

    println!("[L2CS-Net] Chargement de la session ONNX depuis {}...", model_path.display());
    let env = ort::init()
        .with_name("L2CSNet")
        .build()
        .map_err(|e| format!("Erreur init environnement ONNX: {e}"))?;

    let session = ort::session::Session::builder(&env)
        .map_err(|e| format!("Erreur builder ONNX: {e}"))?
        .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
        .map_err(|e| format!("Erreur optimisation: {e}"))?
        .commit_from_file(&model_path)
        .map_err(|e| format!("Erreur chargement session ONNX: {e}"))?;

    *guard = Some(session);
    NEURAL_MODEL_LOADED.store(true, Ordering::Relaxed);
    println!("[L2CS-Net] Session ONNX initialisée avec succès !");
    Ok(())
}

/// Décode une sortie de tenseur L2CS-Net : soit un angle scalaire (en radians ou degrés),
/// soit des logits binnés Softmax (90 bins) issus du modèle ResNet-50 Gaze360
pub fn decode_l2cs_tensor_or_logits(slice: &[f32]) -> f32 {
    if slice.is_empty() {
        return 0.0;
    }
    if slice.len() == 1 {
        let val = slice[0];
        // Py-Feat / onnxscript produit des radians (typiquement dans [-pi, pi])
        if val.abs() <= std::f32::consts::PI + 0.1 {
            val.to_degrees()
        } else {
            val
        }
    } else if slice.len() >= L2CS_NUM_BINS {
        decode_l2cs_binned_softmax(slice)
    } else {
        slice[0]
    }
}

/// Exécute l'inférence L2CS-Net sur un tenseur [1, 3, 224, 224] et extrait (pitch_deg, yaw_deg)
pub fn run_l2cs_inference(input_tensor: &[f32]) -> Option<(f32, f32)> {
    if let Err(e) = get_or_init_l2cs_session() {
        static SHOWN_ERR: AtomicBool = AtomicBool::new(false);
        if !SHOWN_ERR.swap(true, Ordering::Relaxed) {
            eprintln!("[L2CS-Net] Échec initialisation session: {e}");
        }
        return None;
    }
    let mut guard = L2CS_SESSION.lock().ok()?;
    let session = guard.as_mut()?;

    let input_tensor_val = match ort::value::Tensor::from_array((
        [1usize, 3usize, L2CS_INPUT_SIZE, L2CS_INPUT_SIZE],
        input_tensor.to_vec(),
    )) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[L2CS-Net] Erreur création tenseur d'entrée: {e}");
            return None;
        }
    };

    // Ordre natif PyTorch / L2CS-Net : Output 0 = Pitch (fc_pitch), Output 1 = Yaw (fc_yaw)
    let mut p_idx = 0usize;
    let mut y_idx = 1usize;
    for (i, out_meta) in session.outputs().iter().enumerate() {
        let lower = out_meta.name().to_lowercase();
        if lower.contains("pitch") {
            p_idx = i;
        } else if lower.contains("yaw") {
            y_idx = i;
        }
    }

    let outputs = session.run(ort::inputs![input_tensor_val]).ok()?;

    // Gestion flexible des formats d'export ONNX de L2CS-Net / Py-Feat :
    // 1) Deux tenseurs distincts : logits [1, 90] ou scalaires [1, 1] en radians
    if outputs.len() >= 2 {
        let (_, pitch_slice) = outputs[p_idx].try_extract_tensor::<f32>().ok()?;
        let (_, yaw_slice) = outputs[y_idx].try_extract_tensor::<f32>().ok()?;

        let pitch = decode_l2cs_tensor_or_logits(pitch_slice);
        let yaw = decode_l2cs_tensor_or_logits(yaw_slice);
        Some((pitch, yaw))
    } else if outputs.len() == 1 {
        // 2) Tenseur unique combiné : [1, 2] en radians ou [1, 180] logits
        let (_, combined_slice) = outputs[0].try_extract_tensor::<f32>().ok()?;

        if combined_slice.len() == 2 {
            let pitch = decode_l2cs_tensor_or_logits(&combined_slice[0..1]);
            let yaw = decode_l2cs_tensor_or_logits(&combined_slice[1..2]);
            Some((pitch, yaw))
        } else if combined_slice.len() >= 180 {
            let pitch = decode_l2cs_tensor_or_logits(&combined_slice[..90]);
            let yaw = decode_l2cs_tensor_or_logits(&combined_slice[90..180]);
            Some((pitch, yaw))
        } else {
            None
        }
    } else {
        None
    }
}

pub const L2CS_INPUT_SIZE: usize = 224;
pub const L2CS_NUM_BINS: usize = 90;

/// Échantillonne un pixel RGB dans n'importe quel format caméra brut
#[inline]
fn sample_pixel_rgb(frame: &[u8], width: usize, height: usize, format: PixelFormat, px: usize, py: usize) -> (u8, u8, u8) {
    let px = px.min(width.saturating_sub(1));
    let py = py.min(height.saturating_sub(1));
    match format {
        PixelFormat::Grayscale | PixelFormat::Nv12 => {
            let v = frame.get(py * width + px).copied().unwrap_or(128);
            (v, v, v)
        }
        PixelFormat::Yuy2 => {
            let v = frame.get((py * width + px) * 2).copied().unwrap_or(128);
            (v, v, v)
        }
        PixelFormat::Rgb24 => {
            let idx = (py * width + px) * 3;
            if idx + 2 < frame.len() {
                (frame[idx], frame[idx + 1], frame[idx + 2])
            } else {
                (128, 128, 128)
            }
        }
        PixelFormat::Rgb32 => {
            let idx = (py * width + px) * 4;
            if idx + 2 < frame.len() {
                (frame[idx + 2], frame[idx + 1], frame[idx])
            } else {
                (128, 128, 128)
            }
        }
    }
}

/// Échantillonne un pixel en interpolation bilinéaire continue (anti-aliasing)
#[inline]
fn sample_pixel_rgb_bilinear(
    frame: &[u8],
    width: usize,
    height: usize,
    format: PixelFormat,
    fx: f32,
    fy: f32,
) -> (f32, f32, f32) {
    let fx = fx.clamp(0.0, width.saturating_sub(1) as f32);
    let fy = fy.clamp(0.0, height.saturating_sub(1) as f32);

    let x0 = fx.floor() as usize;
    let y0 = fy.floor() as usize;
    let x1 = (x0 + 1).min(width.saturating_sub(1));
    let y1 = (y0 + 1).min(height.saturating_sub(1));

    let wx1 = fx - x0 as f32;
    let wx0 = 1.0 - wx1;
    let wy1 = fy - y0 as f32;
    let wy0 = 1.0 - wy1;

    let (r00, g00, b00) = sample_pixel_rgb(frame, width, height, format, x0, y0);
    let (r10, g10, b10) = sample_pixel_rgb(frame, width, height, format, x1, y0);
    let (r01, g01, b01) = sample_pixel_rgb(frame, width, height, format, x0, y1);
    let (r11, g11, b11) = sample_pixel_rgb(frame, width, height, format, x1, y1);

    let r = wy0 * (wx0 * r00 as f32 + wx1 * r10 as f32) + wy1 * (wx0 * r01 as f32 + wx1 * r11 as f32);
    let g = wy0 * (wx0 * g00 as f32 + wx1 * g10 as f32) + wy1 * (wx0 * g01 as f32 + wx1 * g11 as f32);
    let b = wy0 * (wx0 * b00 as f32 + wx1 * b10 as f32) + wy1 * (wx0 * b01 as f32 + wx1 * b11 as f32);

    (r, g, b)
}

static LAST_FACE_RECT: Mutex<Option<(f32, f32, f32, u32)>> = Mutex::new(None);

/// Localise la boîte englobante serrée du visage par analyse de chrominance et variance spatiale
/// avec calage anatomique pour que les yeux se situent précisément à ~38% du haut du cadrage.
pub fn detect_face_region(
    frame: &[u8],
    width: usize,
    height: usize,
    format: PixelFormat,
) -> (usize, usize, usize, usize) {
    let def_cx = width as f32 * 0.5;
    let def_cy = height as f32 * 0.45;
    let def_size = (height as f32 * 0.52).min(width as f32 * 0.52).max(110.0);

    let step = 6;
    let mut sum_x = 0.0f32;
    let mut sum_y = 0.0f32;
    let mut sum_x2 = 0.0f32;
    let mut sum_y2 = 0.0f32;
    let mut weight_sum = 0.0f32;
    let mut count = 0u32;

    for y in (height / 10..9 * height / 10).step_by(step) {
        let dy = (y as f32 - def_cy) / (height as f32 * 0.5);
        for x in (width / 10..9 * width / 10).step_by(step) {
            let (r, g, b) = sample_pixel_rgb(frame, width, height, format, x, y);
            
            // Modèle de peau en chrominance normalisée (invariant à l'éclairage et aux ombres)
            let rf = r as f32;
            let gf = g as f32;
            let bf = b as f32;
            let total = rf + gf + bf;
            let is_skin = if total >= 50.0 && total <= 740.0 {
                let rn = rf / total;
                let gn = gf / total;
                (rn > 0.35 && rn < 0.58 && gn > 0.24 && gn < 0.39 && (rn - gn) >= 0.03)
                    || (r > 45 && g > 25 && b > 15 && r > g && (r as i32 - b as i32) >= 8)
            } else {
                false
            };
            if is_skin {
                let dx = (x as f32 - def_cx) / (width as f32 * 0.5);
                let w = (1.0 - (dx * dx + dy * dy) * 0.6).max(0.1);
                sum_x += x as f32 * w;
                sum_y += y as f32 * w;
                sum_x2 += (x as f32).powi(2) * w;
                sum_y2 += (y as f32).powi(2) * w;
                weight_sum += w;
                count += 1;
            }
        }
    }

    let detected = count > 18 && weight_sum > 8.0;
    let (raw_cx, raw_cy, raw_size) = if detected {
        let skin_cx = sum_x / weight_sum;
        let skin_cy = sum_y / weight_sum;
        
        // Variance exacte E[X^2] - E[X]^2 centrée sur le visage (ne dépend pas du centre écran)
        let var_x = (sum_x2 / weight_sum - skin_cx.powi(2)).max(1.0);
        let var_y = (sum_y2 / weight_sum - skin_cy.powi(2)).max(1.0);
        let std_x = var_x.sqrt();
        let std_y = var_y.sqrt();

        let spread = (std_x.max(std_y) * 2.8).clamp(110.0, height as f32 * 0.85);

        // Estimation de la position des yeux (~10% au-dessus du centre de masse de peau)
        let estimated_eye_y = skin_cy - spread * 0.10;
        // Cadrage anatomique : yeux positionnés à 38% du haut du crop (box_top = eye_y - 0.38 * spread)
        // Le centre vertical de la boîte est alors : estimated_eye_y + 0.12 * spread
        let box_cy = estimated_eye_y + spread * 0.12;
        (skin_cx, box_cy, spread)
    } else {
        let eye_y = def_cy - def_size * 0.12;
        let box_cy = eye_y + def_size * 0.12;
        (def_cx, box_cy, def_size)
    };

    let (cx, cy, size) = if let Ok(mut g) = LAST_FACE_RECT.lock() {
        if let Some((px, py, ps, missed)) = *g {
            if detected {
                // Amortissement temporel fort (88%/12%) pour éliminer tout tremblement du cadre
                let ncx = px * 0.88 + raw_cx * 0.12;
                let ncy = py * 0.88 + raw_cy * 0.12;
                let ns = ps * 0.90 + raw_size * 0.10;
                *g = Some((ncx, ncy, ns, 0));
                (ncx, ncy, ns)
            } else if missed < 30 {
                // Perte ponctuelle (flou, mouvement brusque) : maintien ferme de la dernière position
                *g = Some((px, py, ps, missed + 1));
                (px, py, ps)
            } else {
                // Perte prolongée (> 1 sec) : recentrage progressif
                let ncx = px * 0.95 + def_cx * 0.05;
                let ncy = py * 0.95 + def_cy * 0.05;
                let ns = ps * 0.95 + def_size * 0.05;
                *g = Some((ncx, ncy, ns, missed.min(100)));
                (ncx, ncy, ns)
            }
        } else {
            *g = Some((raw_cx, raw_cy, raw_size, if detected { 0 } else { 1 }));
            (raw_cx, raw_cy, raw_size)
        }
    } else {
        (raw_cx, raw_cy, raw_size)
    };

    let half = size * 0.5;
    let min_x = (cx - half).max(0.0).min(width.saturating_sub(1) as f32) as usize;
    let max_x = (cx + half).max(0.0).min(width.saturating_sub(1) as f32) as usize;
    let min_y = (cy - half).max(0.0).min(height.saturating_sub(1) as f32) as usize;
    let max_y = (cy + half).max(0.0).min(height.saturating_sub(1) as f32) as usize;

    (min_x, max_x, min_y, max_y)
}

/// Prétraitement avec interpolation bilinéaire anti-aliasing vers le tenseur NCHW [1, 3, 224, 224] normalisé ImageNet
pub fn direct_preprocess_l2cs(
    frame: &[u8],
    width: usize,
    height: usize,
    format: PixelFormat,
    face_roi: (usize, usize, usize, usize),
) -> Vec<f32> {
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];

    let (min_x, max_x, min_y, max_y) = face_roi;
    let roi_w = (max_x.saturating_sub(min_x)).max(1) as f32;
    let roi_h = (max_y.saturating_sub(min_y)).max(1) as f32;

    let plane_size = L2CS_INPUT_SIZE * L2CS_INPUT_SIZE;
    let mut tensor = vec![0.0f32; 3 * plane_size];

    // Rehaussement de contraste dynamique pour caméras floues ou sous-exposées
    let mut min_val = 255u8;
    let mut max_val = 0u8;
    let sample_steps = 16;
    for sy in 0..sample_steps {
        let py = min_y + (sy * (max_y.saturating_sub(min_y))) / sample_steps;
        for sx in 0..sample_steps {
            let px = min_x + (sx * (max_x.saturating_sub(min_x))) / sample_steps;
            let (r, g, b) = sample_pixel_rgb(frame, width, height, format, px, py);
            min_val = min_val.min(r).min(g).min(b);
            max_val = max_val.max(r).max(g).max(b);
        }
    }
    let range = (max_val as f32 - min_val as f32).max(25.0);
    let contrast_scale = 255.0 / range;

    let step_x = roi_w / L2CS_INPUT_SIZE as f32;
    let step_y = roi_h / L2CS_INPUT_SIZE as f32;

    for out_y in 0..L2CS_INPUT_SIZE {
        let fy = min_y as f32 + (out_y as f32 + 0.5) * step_y;
        for out_x in 0..L2CS_INPUT_SIZE {
            let fx = min_x as f32 + (out_x as f32 + 0.5) * step_x;
            let (r_f255, g_f255, b_f255) = sample_pixel_rgb_bilinear(frame, width, height, format, fx, fy);

            let r_stretched = ((r_f255 - min_val as f32) * contrast_scale).clamp(0.0, 255.0);
            let g_stretched = ((g_f255 - min_val as f32) * contrast_scale).clamp(0.0, 255.0);
            let b_stretched = ((b_f255 - min_val as f32) * contrast_scale).clamp(0.0, 255.0);

            let r_f = r_stretched / 255.0;
            let g_f = g_stretched / 255.0;
            let b_f = b_stretched / 255.0;

            let pixel_idx = out_y * L2CS_INPUT_SIZE + out_x;
            tensor[pixel_idx] = (r_f - MEAN[0]) / STD[0];
            tensor[plane_size + pixel_idx] = (g_f - MEAN[1]) / STD[1];
            tensor[plane_size * 2 + pixel_idx] = (b_f - MEAN[2]) / STD[2];
        }
    }

    tensor
}

/// Génère la vignette de prévisualisation à partir de l'image BRUTE complète de la caméra avec
/// cadrage dynamique du visage et réticules oculaires suivant les mouvements de tête
pub fn build_raw_camera_preview(
    frame: &[u8],
    width: usize,
    height: usize,
    format: PixelFormat,
    face_roi: (usize, usize, usize, usize),
    norm_x: f32,
    norm_y: f32,
) -> (Vec<u8>, usize, usize, (f32, f32), (f32, f32)) {
    let target_w = 320usize;
    let target_h = ((target_w as f32 * (height as f32 / width.max(1) as f32)).round() as usize).clamp(180, 260);

    let mut preview = vec![255u8; target_w * target_h * 4];
    let sx_scale = width as f32 / target_w as f32;
    let sy_scale = height as f32 / target_h as f32;

    for ty in 0..target_h {
        let py = ((ty as f32 + 0.5) * sy_scale) as usize;
        for tx in 0..target_w {
            let px = ((tx as f32 + 0.5) * sx_scale) as usize;
            let (r, g, b) = sample_pixel_rgb(frame, width, height, format, px, py);
            let idx = (ty * target_w + tx) * 4;
            preview[idx] = r;
            preview[idx + 1] = g;
            preview[idx + 2] = b;
            preview[idx + 3] = 255;
        }
    }

    // Projection de la boîte visage sur la vue globale
    let to_target_x = |x: f32| -> f32 { (x / width as f32) * target_w as f32 };
    let to_target_y = |y: f32| -> f32 { (y / height as f32) * target_h as f32 };

    let fx0 = to_target_x(face_roi.0 as f32);
    let fx1 = to_target_x(face_roi.1 as f32);
    let fy0 = to_target_y(face_roi.2 as f32);
    let fy1 = to_target_y(face_roi.3 as f32);
    let fw = fx1 - fx0;
    let fh = fy1 - fy0;

    // Orbites oculaires anatomiques relatives au visage
    let base_lx = fx0 + fw * 0.35;
    let base_ly = fy0 + fh * 0.38;
    let base_rx = fx0 + fw * 0.65;
    let base_ry = fy0 + fh * 0.38;

    // Déplacement dynamique des pupilles dans l'orbite selon la direction du regard
    let pupil_travel_x = fw * 0.07;
    let pupil_travel_y = fh * 0.05;

    let left_pupil = (base_lx + norm_x * pupil_travel_x, base_ly + norm_y * pupil_travel_y);
    let right_pupil = (base_rx + norm_x * pupil_travel_x, base_ry + norm_y * pupil_travel_y);

    // Encadré discret du visage (Bleu/Cyan)
    let box_color = [0, 180, 240, 180];
    let ix0 = (fx0.round() as isize).clamp(0, target_w as isize - 1) as usize;
    let ix1 = (fx1.round() as isize).clamp(0, target_w as isize - 1) as usize;
    let iy0 = (fy0.round() as isize).clamp(0, target_h as isize - 1) as usize;
    let iy1 = (fy1.round() as isize).clamp(0, target_h as isize - 1) as usize;
    for x in ix0..=ix1 {
        for y in [iy0, iy1] {
            let idx = (y * target_w + x) * 4;
            preview[idx..idx + 4].copy_from_slice(&box_color);
        }
    }
    for y in iy0..=iy1 {
        for x in [ix0, ix1] {
            let idx = (y * target_w + x) * 4;
            preview[idx..idx + 4].copy_from_slice(&box_color);
        }
    }

    // Réticules oculaires verts qui suivent le regard de chaque pupille
    let reticle_color = [0, 230, 118, 255];
    for (ex, ey) in [left_pupil, right_pupil] {
        let icx = ex.round() as isize;
        let icy = ey.round() as isize;
        for d in -6isize..=6isize {
            for (px, py) in [(icx + d, icy), (icx, icy + d)] {
                if px >= 0 && px < target_w as isize && py >= 0 && py < target_h as isize {
                    let idx = (py as usize * target_w + px as usize) * 4;
                    preview[idx..idx + 4].copy_from_slice(&reticle_color);
                }
            }
        }
    }

    // Rayons directeurs 3D du regard (Jaune néon éclatant) partant de chaque pupille
    let ray_len = 45.0f32;
    let ray_color = [255, 238, 0, 255];
    let (vx, vy) = (norm_x, norm_y);
    for (ex, ey) in [left_pupil, right_pupil] {
        let target_x = ex + vx * ray_len;
        let target_y = ey + vy * ray_len;
        for step in 0..=24 {
            let t = step as f32 / 24.0;
            let px = (ex + (target_x - ex) * t).round() as isize;
            let py = (ey + (target_y - ey) * t).round() as isize;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let rx = px + dx;
                    let ry = py + dy;
                    if rx >= 0 && rx < target_w as isize && ry >= 0 && ry < target_h as isize {
                        let idx = (ry as usize * target_w + rx as usize) * 4;
                        preview[idx..idx + 4].copy_from_slice(&ray_color);
                    }
                }
            }
        }
    }

    (preview, target_w, target_h, left_pupil, right_pupil)
}

/// Décode les logits binnés de L2CS-Net par espérance mathématique Softmax :
/// angle en degrés = \sum (Softmax(logits)_i * i * 4) - 180°
pub fn decode_l2cs_binned_softmax(logits: &[f32]) -> f32 {
    if logits.is_empty() {
        return 0.0;
    }
    let n = logits.len().min(L2CS_NUM_BINS);
    let max_val = logits[..n].iter().copied().fold(f32::NEG_INFINITY, f32::max);

    let mut sum_exp = 0.0f32;
    let exps: Vec<f32> = logits[..n].iter().map(|&v| {
        let e = (v - max_val).exp();
        sum_exp += e;
        e
    }).collect();

    let sum_exp_inv = if sum_exp > 1e-6 { 1.0 / sum_exp } else { 0.0 };
    let mut expectation = 0.0f32;
    for (i, &exp_v) in exps.iter().enumerate() {
        let prob = exp_v * sum_exp_inv;
        expectation += prob * (i as f32);
    }

    // Discrétisation standard L2CS : 90 bins de 4° couvrant [-180°, +180°]
    expectation * 4.0 - 180.0
}

/// Génère une vignette de débogage simulée détourée des yeux lorsque la caméra physique n'est pas active
fn update_fallback_debug_state(norm_x: f32, norm_y: f32) {
    let eye_w = 160usize;
    let eye_h = 120usize;
    let sep_w = 6usize;
    let total_w = eye_w * 2 + sep_w;
    let total_h = eye_h;

    let mut out = vec![22u8; total_w * total_h * 4];
    for pixel in out.chunks_exact_mut(4) {
        pixel[3] = 255;
    }

    let left_center = (80.0f32, 60.0f32);
    let right_center = (80.0f32 + (eye_w + sep_w) as f32, 60.0f32);

    let lx = left_center.0 + norm_x * 24.0;
    let ly = left_center.1 + norm_y * 18.0;
    let rx = right_center.0 + norm_x * 24.0;
    let ry = right_center.1 + norm_y * 18.0;

    // Rendu synthétique des deux globes oculaires détourés avec sclère, iris et pupille
    for &(base_cx, base_cy, px, py) in &[(left_center.0, left_center.1, lx, ly), (right_center.0, right_center.1, rx, ry)] {
        for y in (base_cy as usize).saturating_sub(44)..=(base_cy as usize + 44).min(total_h - 1) {
            for x in (base_cx as usize).saturating_sub(60)..=(base_cx as usize + 60).min(total_w - 1) {
                let ex = (x as f32 - base_cx) / 58.0;
                let ey = (y as f32 - base_cy) / 36.0;
                if ex * ex + ey * ey <= 1.0 {
                    let idx = (y * total_w + x) * 4;
                    // Sclère
                    out[idx] = 228;
                    out[idx + 1] = 232;
                    out[idx + 2] = 238;

                    let idist = (x as f32 - px).hypot(y as f32 - py);
                    // Iris
                    if idist <= 22.0 {
                        out[idx] = 40;
                        out[idx + 1] = 115;
                        out[idx + 2] = 185;
                    }
                    // Pupille
                    if idist <= 10.5 {
                        out[idx] = 16;
                        out[idx + 1] = 16;
                        out[idx + 2] = 20;
                    }
                    // Reflet spéculaire
                    if (x as f32 - (px - 4.0)).hypot(y as f32 - (py - 4.0)) <= 3.0 {
                        out[idx] = 255;
                        out[idx + 1] = 255;
                        out[idx + 2] = 255;
                    }
                }
            }
        }
    }

    // Séparateur vertical central
    let sep_start = eye_w;
    let sep_end = eye_w + sep_w;
    for y in 0..total_h {
        for x in sep_start..sep_end {
            let idx = (y * total_w + x) * 4;
            let c = if x == sep_start + sep_w / 2 { 60 } else { 15 };
            out[idx] = c;
            out[idx + 1] = c;
            out[idx + 2] = c;
            out[idx + 3] = 255;
        }
    }

    // Cadres verts autour de chaque œil détouré
    let green = [0, 230, 118, 255];
    for thickness in 0..2 {
        // Œil gauche
        for x in 0..eye_w {
            let top_idx = (thickness * total_w + x) * 4;
            let bot_idx = ((total_h - 1 - thickness) * total_w + x) * 4;
            out[top_idx..top_idx + 4].copy_from_slice(&green);
            out[bot_idx..bot_idx + 4].copy_from_slice(&green);
        }
        for y in 0..total_h {
            let left_idx = (y * total_w + thickness) * 4;
            let right_idx = (y * total_w + (eye_w - 1 - thickness)) * 4;
            out[left_idx..left_idx + 4].copy_from_slice(&green);
            out[right_idx..right_idx + 4].copy_from_slice(&green);
        }
        // Œil droit
        for x in sep_end..total_w {
            let top_idx = (thickness * total_w + x) * 4;
            let bot_idx = ((total_h - 1 - thickness) * total_w + x) * 4;
            out[top_idx..top_idx + 4].copy_from_slice(&green);
            out[bot_idx..bot_idx + 4].copy_from_slice(&green);
        }
        for y in 0..total_h {
            let left_idx = (y * total_w + (sep_end + thickness)) * 4;
            let right_idx = (y * total_w + (total_w - 1 - thickness)) * 4;
            out[left_idx..left_idx + 4].copy_from_slice(&green);
            out[right_idx..right_idx + 4].copy_from_slice(&green);
        }
    }

    // Réticules sur les pupilles
    let reticle_color = [255, 45, 65, 255];
    let core_color = [255, 255, 255, 255];
    for &(cx, cy, min_bx, max_bx) in &[(lx, ly, 0, eye_w as isize), (rx, ry, sep_end as isize, total_w as isize)] {
        let icx = cx.round() as isize;
        let icy = cy.round() as isize;
        for d in -10isize..=10isize {
            if d.abs() > 2 {
                for (px, py) in [(icx + d, icy), (icx, icy + d)] {
                    if px >= min_bx && px < max_bx && py >= 0 && py < total_h as isize {
                        let idx = (py as usize * total_w + px as usize) * 4;
                        out[idx..idx + 4].copy_from_slice(&reticle_color);
                    }
                }
            }
        }
        for (px, py) in [(icx, icy), (icx + 1, icy), (icx - 1, icy), (icx, icy + 1), (icx, icy - 1)] {
            if px >= min_bx && px < max_bx && py >= 0 && py < total_h as isize {
                let idx = (py as usize * total_w + px as usize) * 4;
                out[idx..idx + 4].copy_from_slice(&core_color);
            }
        }
    }

    // Rayons directeurs jaunes (vecteurs de détection)
    let ray_len = 35.0f32;
    let yellow = [255, 238, 0, 255];
    for &(px, py) in &[(lx, ly), (rx, ry)] {
        let target_x = px + norm_x * ray_len;
        let target_y = py + norm_y * ray_len;
        for step in 0..=24 {
            let t = step as f32 / 24.0;
            let rx = (px + (target_x - px) * t).round() as isize;
            let ry = (py + (target_y - py) * t).round() as isize;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let x = rx + dx;
                    let y = ry + dy;
                    if x >= 0 && x < total_w as isize && y >= 0 && y < total_h as isize {
                        let idx = (y as usize * total_w + x as usize) * 4;
                        out[idx..idx + 4].copy_from_slice(&yellow);
                    }
                }
            }
        }
    }

    set_eye_debug_state(EyeDebugState {
        preview_crop: out,
        width: total_w,
        height: total_h,
        left_pupil: Some((lx, ly)),
        right_pupil: Some((rx, ry)),
        left_box: (0, eye_w, 0, eye_h),
        right_box: (sep_end, total_w, 0, eye_h),
        timestamp: Instant::now(),
    });
}

/// Analyse une image vidéo brute en appelant directement le modèle neuronal ONNX L2CS-Net
pub fn extract_pupil_and_head_pose(
    frame: &[u8],
    width: usize,
    height: usize,
    format: PixelFormat,
) -> Option<RawGazeMeasurement> {
    if width < 64 || height < 48 || frame.is_empty() {
        return None;
    }

    // 1. Priorité absolue : Inférence neuronale directe L2CS-Net sans pré/post-traitement intermédiaire
    if is_eyetracking_model_installed() {
        let face_roi = detect_face_region(frame, width, height, format);
        let tensor = direct_preprocess_l2cs(frame, width, height, format, face_roi);

        if let Some((pitch_deg, yaw_deg)) = run_l2cs_inference(&tensor) {
            // Webcam au sommet de l'écran : calibrage angulaire pour un écran de bureau à ~60cm
            let norm_x = (yaw_deg / 15.0).clamp(-1.0, 1.0);

            // Repère pitch caméra : pitch négatif = regard vers le bas
            // Normalisation dynamique sans butée artificielle de +45°
            let pitch_offset = get_camera_pitch_offset();
            let compensated_pitch = pitch_deg + pitch_offset;
            // Échelle angulaire naturelle décisive : amplitude verticale humaine ~11.5° (au lieu de 16°)
            let norm_y = (compensated_pitch / 11.5).clamp(-1.0, 1.0);

            // Construction du retour vidéo brut complet avec annotations dynamiques
            let (preview, prev_w, prev_h, left_eye, right_eye) =
                build_raw_camera_preview(frame, width, height, format, face_roi, norm_x, norm_y);

            let measurement = RawGazeMeasurement {
                eye_x: norm_x,
                eye_y: norm_y,
                head_yaw: (yaw_deg / 35.0).clamp(-1.0, 1.0),
                head_pitch: compensated_pitch,
                timestamp: Instant::now(),
            };

            set_eye_debug_state(EyeDebugState {
                preview_crop: preview,
                width: prev_w,
                height: prev_h,
                left_pupil: Some(left_eye),
                right_pupil: Some(right_eye),
                left_box: (0, prev_w / 2, 0, prev_h),
                right_box: (prev_w / 2, prev_w, 0, prev_h),
                timestamp: Instant::now(),
            });

            static LAST_L2CS_LOG: Mutex<Option<Instant>> = Mutex::new(None);
            if is_dev_mode() {
                if let Ok(mut lock) = LAST_L2CS_LOG.lock() {
                    let should_log = lock.map_or(true, |t| t.elapsed() >= Duration::from_millis(1500));
                    if should_log {
                        *lock = Some(Instant::now());
                        println!(
                            "[Eye-Tracking Dev] [L2CS-Net Direct] PitchBrut={:+.1}° PitchComp={:+.1}° Yaw={:+.1}° -> Gaze=({:+.2}, {:+.2})",
                            pitch_deg, compensated_pitch, yaw_deg, norm_x, norm_y
                        );
                    }
                }
            }

            return Some(measurement);
        }
    }

    // Repli direct : extraction pupillaire par intensité sombre au sein des orbites oculaires
    let face_roi = detect_face_region(frame, width, height, format);
    let (min_x, max_x, min_y, max_y) = face_roi;
    let fw = (max_x.saturating_sub(min_x)).max(1) as f32;
    let fh = (max_y.saturating_sub(min_y)).max(1) as f32;

    let lx0 = (min_x as f32 + fw * 0.20) as usize;
    let lx1 = (min_x as f32 + fw * 0.44) as usize;
    let ly0 = (min_y as f32 + fh * 0.28) as usize;
    let ly1 = (min_y as f32 + fh * 0.48) as usize;

    let rx0 = (min_x as f32 + fw * 0.56) as usize;
    let rx1 = (min_x as f32 + fw * 0.80) as usize;
    let ry0 = ly0;
    let ry1 = ly1;

    let find_darkest_center = |x0: usize, x1: usize, y0: usize, y1: usize| -> (f32, f32) {
        let mut min_val = 255u32;
        let mut best_x = (x0 + x1) / 2;
        let mut best_y = (y0 + y1) / 2;
        for y in y0..y1.min(height) {
            for x in x0..x1.min(width) {
                let (r, g, b) = sample_pixel_rgb(frame, width, height, format, x, y);
                let lum = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
                if lum < min_val {
                    min_val = lum;
                    best_x = x;
                    best_y = y;
                }
            }
        }
        (best_x as f32, best_y as f32)
    };

    let (l_dark_x, l_dark_y) = find_darkest_center(lx0, lx1, ly0, ly1);
    let (r_dark_x, r_dark_y) = find_darkest_center(rx0, rx1, ry0, ry1);

    let l_mid_x = (lx0 + lx1) as f32 * 0.5;
    let l_mid_y = (ly0 + ly1) as f32 * 0.5;
    let r_mid_x = (rx0 + rx1) as f32 * 0.5;
    let r_mid_y = (ry0 + ry1) as f32 * 0.5;

    let dx_l = (l_dark_x - l_mid_x) / (fw * 0.12).max(1.0);
    let dy_l = (l_dark_y - l_mid_y) / (fh * 0.10).max(1.0);
    let dx_r = (r_dark_x - r_mid_x) / (fw * 0.12).max(1.0);
    let dy_r = (r_dark_y - r_mid_y) / (fh * 0.10).max(1.0);

    let norm_x = ((dx_l + dx_r) * 0.5).clamp(-1.0, 1.0);
    // Compensation fixe dans le fallback pupillaire :
    let baseline_down_offset = 0.45;
    let norm_y = ((((dy_l + dy_r) * 0.5) - baseline_down_offset) * 1.85).clamp(-1.0, 1.0);

    let (preview, prev_w, prev_h, left_eye, right_eye) =
        build_raw_camera_preview(frame, width, height, format, face_roi, norm_x, norm_y);

    set_eye_debug_state(EyeDebugState {
        preview_crop: preview,
        width: prev_w,
        height: prev_h,
        left_pupil: Some(left_eye),
        right_pupil: Some(right_eye),
        left_box: (0, prev_w / 2, 0, prev_h),
        right_box: (prev_w / 2, prev_w, 0, prev_h),
        timestamp: Instant::now(),
    });

    Some(RawGazeMeasurement {
        eye_x: norm_x,
        eye_y: norm_y,
        head_yaw: 0.0,
        head_pitch: 0.0,
        timestamp: Instant::now(),
    })
}

// ============================================================================
// 7b. Pilotes Réseau Matériels : OpenGaze (Tobii/GazePoint) & OpenTrack (UDP)
// ============================================================================

/// Extrait un attribut float d'une balise XML OpenGaze (ex: FPOGX="0.5123")
fn parse_opengaze_xml_attr(line: &str, attr: &str) -> Option<f32> {
    let pattern = format!("{attr}=\"");
    if let Some(pos) = line.find(&pattern) {
        let rest = &line[pos + pattern.len()..];
        if let Some(end) = rest.find('"') {
            return rest[..end].trim().parse::<f32>().ok();
        }
    }
    None
}

/// Boucle client TCP pour le protocole OpenGaze (GazePoint GP3, Tobii OpenGaze bridge, Pupil Labs)
fn run_opengaze_client_loop(addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("[Hardware Eye Tracker] Connexion au serveur OpenGaze sur {addr}...");
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;

    // Initialisation du flux OpenGaze standard
    let init_cmds = "<SET ID=\"ENABLE_SEND_POG_FIX\" STATE=\"1\" />\r\n<SET ID=\"ENABLE_SEND_DATA\" STATE=\"1\" />\r\n";
    stream.write_all(init_cmds.as_bytes())?;
    stream.flush()?;

    EYE_HARDWARE_CONNECTED.store(true, Ordering::SeqCst);
    set_camera_device_name("GazePoint / Tobii OpenGaze");
    println!("[Hardware Eye Tracker] Connecté au flux matériel OpenGaze !");

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let mut frame_count = 0u32;
    let mut last_hz_time = Instant::now();

    while is_camera_worker_running() {
        let active = is_eye_tracking_enabled() || is_calibration_active() || is_dev_mode() || is_eye_debug_overlay_visible();
        if !active {
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }
        if check_and_stop_eye_tracking_on_escape() {
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }

        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // Déconnexion du serveur
            Ok(_) => {
                // Analyse du paquet <REC FPOGX="..." FPOGY="..." FPOGV="1" />
                if line.contains("<REC") {
                    let x_opt = parse_opengaze_xml_attr(&line, "FPOGX").or_else(|| parse_opengaze_xml_attr(&line, "BPOGX"));
                    let y_opt = parse_opengaze_xml_attr(&line, "FPOGY").or_else(|| parse_opengaze_xml_attr(&line, "BPOGY"));
                    let valid = parse_opengaze_xml_attr(&line, "FPOGV").or_else(|| parse_opengaze_xml_attr(&line, "BPOGV")).unwrap_or(1.0) > 0.5;

                    if valid {
                        if let (Some(x), Some(y)) = (x_opt, y_opt) {
                            // Conversion des coordonnées normalisées de l'écran [0.0..1.0] -> [-1.0..+1.0]
                            let norm_x = ((x - 0.5) * 2.0).clamp(-1.0, 1.0);
                            let norm_y = ((y - 0.5) * 2.0).clamp(-1.0, 1.0);

                            set_latest_raw_gaze(RawGazeMeasurement {
                                eye_x: norm_x,
                                eye_y: norm_y,
                                head_yaw: 0.0,
                                head_pitch: 0.0,
                                timestamp: Instant::now(),
                            });
                            update_fallback_debug_state(norm_x, norm_y);

                            frame_count += 1;
                            if last_hz_time.elapsed() >= Duration::from_secs(1) {
                                EYE_HARDWARE_HZ.store(frame_count, Ordering::Relaxed);
                                frame_count = 0;
                                last_hz_time = Instant::now();
                            }
                        }
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock => {
                continue;
            }
            Err(_) => break,
        }
    }

    EYE_HARDWARE_CONNECTED.store(false, Ordering::SeqCst);
    EYE_HARDWARE_HZ.store(0, Ordering::Relaxed);
    Ok(())
}

/// Boucle réceptrice UDP pour le protocole OpenTrack / FreeTrack (port 4242)
fn run_opentrack_udp_loop(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    println!("[Hardware Eye Tracker] Écoute des paquets OpenTrack UDP sur 0.0.0.0:{port}...");
    let socket = UdpSocket::bind(format!("0.0.0.0:{port}"))?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;

    EYE_HARDWARE_CONNECTED.store(true, Ordering::SeqCst);
    set_camera_device_name("OpenTrack / FreeTrack (UDP)");

    let mut buf = [0u8; 128];
    let mut frame_count = 0u32;
    let mut last_hz_time = Instant::now();

    while is_camera_worker_running() {
        let active = is_eye_tracking_enabled() || is_calibration_active() || is_dev_mode() || is_eye_debug_overlay_visible();
        if !active {
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }
        if check_and_stop_eye_tracking_on_escape() {
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }

        match socket.recv_from(&mut buf) {
            Ok((len, _src)) if len >= 48 => {
                // 6 doubles (little endian) : [x, y, z, yaw, pitch, roll]
                let mut doubles = [0.0f64; 6];
                for i in 0..6 {
                    let b = &buf[i * 8..(i + 1) * 8];
                    doubles[i] = f64::from_le_bytes(b.try_into().unwrap_or_default());
                }
                let yaw = doubles[3] as f32;
                let pitch = doubles[4] as f32;

                // Conversion angulaire en coordonnées normalisées de regard
                let norm_x = (yaw / 18.0).clamp(-1.0, 1.0);
                let norm_y = (-pitch / 11.5).clamp(-1.0, 1.0);

                set_latest_raw_gaze(RawGazeMeasurement {
                    eye_x: norm_x,
                    eye_y: norm_y,
                    head_yaw: yaw,
                    head_pitch: pitch,
                    timestamp: Instant::now(),
                });
                update_fallback_debug_state(norm_x, norm_y);

                frame_count += 1;
                if last_hz_time.elapsed() >= Duration::from_secs(1) {
                    EYE_HARDWARE_HZ.store(frame_count, Ordering::Relaxed);
                    frame_count = 0;
                    last_hz_time = Instant::now();
                }
            }
            Ok(_) => continue,
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock => {
                continue;
            }
            Err(_) => break,
        }
    }

    EYE_HARDWARE_CONNECTED.store(false, Ordering::SeqCst);
    EYE_HARDWARE_HZ.store(0, Ordering::Relaxed);
    Ok(())
}

// ============================================================================
// 7. Pilote d'Acquisition Vidéo Nokhwa (Moderne, Multiplateforme)
// ============================================================================

fn run_camera() -> Result<(), Box<dyn std::error::Error>> {
    let mut camera_opt = None;
    let mut last_err = String::new();

    // Détection des caméras réelles connectées, avec repli sur les index 0..4
    let candidate_indices: Vec<CameraIndex> = query(ApiBackend::Auto)
        .ok()
        .map(|list| list.into_iter().map(|info| info.index().clone()).collect())
        .filter(|v: &Vec<CameraIndex>| !v.is_empty())
        .unwrap_or_else(|| (0..4).map(CameraIndex::Index).collect());

    // Formats tolérants par ordre de préférence décroissant
    let requested_formats = [
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::None),
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::HighestResolution(Resolution::new(640, 480))),
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::HighestResolution(Resolution::new(1280, 720))),
    ];

    'probe: for cam_idx in candidate_indices {
        for req_fmt in &requested_formats {
            match Camera::new(cam_idx.clone(), req_fmt.clone()) {
                Ok(mut cam) => match cam.open_stream() {
                    Ok(()) => {
                        camera_opt = Some(cam);
                        break 'probe;
                    }
                    Err(e) => {
                        last_err = format!("open_stream({}): {}", cam_idx, e);
                    }
                },
                Err(e) => {
                    last_err = format!("Camera::new({}): {}", cam_idx, e);
                }
            }
        }
    }

    let mut camera = match camera_opt {
        Some(c) => c,
        None => return Err(format!("Aucune caméra accessible (dernière erreur: {last_err})").into()),
    };

    let dev_name = camera.info().human_name();
    set_camera_device_name(&dev_name);
    println!("[Eye-Tracking] Caméra connectée : {}", dev_name);

    let frame_delay = Duration::from_millis(33); // ~30 FPS
    let mut consecutive_errors: u32 = 0;

    while is_camera_worker_running() {
        let active = is_eye_tracking_enabled()
            || is_calibration_active()
            || is_dev_mode()
            || is_eye_debug_overlay_visible();
        if !active {
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }

        if check_and_stop_eye_tracking_on_escape() {
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }

        let loop_start = Instant::now();
        let frame = match camera.frame() {
            Ok(f) => {
                consecutive_errors = 0;
                f
            }
            Err(e) => {
                consecutive_errors += 1;
                if consecutive_errors > 45 {
                    return Err(format!("Perte prolongée du flux caméra: {e}").into());
                }
                poll_mouse_fallback_sample();
                std::thread::sleep(Duration::from_millis(30));
                continue;
            }
        };

        // Décodage avec fallback transparent si format direct non-RGB (YUY2, NV12, etc.)
        let decoded_img = frame.decode_image::<RgbFormat>();
        let (raw_bytes, width, height, format) = match &decoded_img {
            Ok(decoded) => (decoded.as_raw().as_slice(), decoded.width() as usize, decoded.height() as usize, PixelFormat::Rgb24),
            Err(_) => {
                let res = frame.resolution();
                let w = res.width() as usize;
                let h = res.height() as usize;
                let buf = frame.buffer();
                let fmt = if buf.len() == w * h * 2 {
                    PixelFormat::Yuy2
                } else if buf.len() == w * h * 4 {
                    PixelFormat::Rgb32
                } else {
                    PixelFormat::Rgb24
                };
                (buf, w, h, fmt)
            }
        };

        if width > 0 && height > 0 && !raw_bytes.is_empty() {
            if let Some(measurement) = extract_pupil_and_head_pose(raw_bytes, width, height, format) {
                set_latest_raw_gaze(measurement);
            }
        }

        let elapsed = loop_start.elapsed();
        if elapsed < frame_delay {
            std::thread::sleep(frame_delay - elapsed);
        }
    }

    let _ = camera.stop_stream();
    Ok(())
}

/// Démarre le thread de capture vidéo en arrière-plan avec protection contre le multi-spawn
pub fn start_camera_worker() {
    let _guard = match CAMERA_START_LOCK.try_lock() {
        Ok(g) => g,
        Err(_) => return, // Un démarrage est déjà en cours
    };

    if is_camera_worker_running() {
        return;
    }

    // Nettoyage non-bloquant de l'ancien thread uniquement s'il est terminé
    if let Ok(mut handle_guard) = CAMERA_WORKER_HANDLE.lock() {
        if let Some(old_handle) = handle_guard.as_ref() {
            if !old_handle.is_finished() {
                return;
            }
        }
        if let Some(old_handle) = handle_guard.take() {
            let _ = old_handle.join();
        }
    }

    CAMERA_WORKER_RUNNING.store(true, Ordering::SeqCst);

    let handle = std::thread::Builder::new()
        .name("eyetracking-camera".to_string())
        .spawn(move || {
            println!("[Eye-Tracking] Démarrage du thread d'acquisition vidéo (nokhwa)...");
            
            let source = get_eye_tracker_source();
            match source {
                EyeTrackerSource::OpenGazeTcp => {
                    let addr = get_eye_hardware_addr();
                    let _ = run_opengaze_client_loop(&addr);
                    return;
                }
                EyeTrackerSource::OpenTrackUdp => {
                    let _ = run_opentrack_udp_loop(4242);
                    return;
                }
                EyeTrackerSource::WebcamNeural => {
                    // Poursuite standard via webcam nokhwa
                }
            }
            
            std::thread::sleep(Duration::from_millis(100));

            match run_camera() {
                Ok(()) => (),
                Err(e) => {
                    eprintln!("[Eye-Tracking] Impossible d'ouvrir la caméra via nokhwa: {e}. Bascule en mode simulation.");
                    run_fallback_camera_loop();
                }
            }
            println!("[Eye-Tracking] Thread d'acquisition vidéo arrêté.");
            // N'éteindre le flag que si aucun nouveau thread n'a été planifié
            if let Ok(guard) = CAMERA_WORKER_HANDLE.lock() {
                if guard.as_ref().map_or(true, |h| h.is_finished()) {
                    CAMERA_WORKER_RUNNING.store(false, Ordering::SeqCst);
                }
            }
        });

    if let (Ok(h), Ok(mut handle_guard)) = (handle, CAMERA_WORKER_HANDLE.lock()) {
        *handle_guard = Some(h);
    }
}

/// Alias de compatibilité
pub fn start_wmf_camera_worker() {
    start_camera_worker();
}

/// Mesure de repli réactive basée sur le curseur système pour permettre le test et calibrage sans webcam
fn poll_mouse_fallback_sample() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let mut pt = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut pt).is_ok() {
            let screen_w = GetSystemMetrics(SM_CXSCREEN).max(800) as f32;
            let screen_h = GetSystemMetrics(SM_CYSCREEN).max(600) as f32;
            let norm_x = (((pt.x as f32 / screen_w) - 0.5) * 2.0).clamp(-1.0, 1.0);
            let norm_y = (((pt.y as f32 / screen_h) - 0.5) * 2.0).clamp(-1.0, 1.0);
            update_fallback_debug_state(norm_x.clamp(-1.0, 1.0), norm_y.clamp(-1.0, 1.0));
            set_latest_raw_gaze(RawGazeMeasurement {
                eye_x: norm_x.clamp(-1.0, 1.0),
                eye_y: norm_y.clamp(-1.0, 1.0),
                head_yaw: 0.0,
                head_pitch: 0.0,
                timestamp: Instant::now(),
            });
            return;
        }
    }

    #[cfg(not(windows))]
    {
        if let Some((x, y)) = crate::window_util::get_global_cursor_pos() {
            let screen_w = 1920.0f32;
            let screen_h = 1080.0f32;
            let norm_x = (((x as f32 / screen_w) - 0.5) * 2.0).clamp(-1.0, 1.0);
            let norm_y = (((y as f32 / screen_h) - 0.5) * 2.0).clamp(-1.0, 1.0);
            update_fallback_debug_state(norm_x, norm_y);
            set_latest_raw_gaze(RawGazeMeasurement {
                eye_x: norm_x,
                eye_y: norm_y,
                head_yaw: 0.0,
                head_pitch: 0.0,
                timestamp: Instant::now(),
            });
            return;
        }

        let sample = RawGazeMeasurement {
            eye_x: 0.0,
            eye_y: 0.0,
            head_yaw: 0.0,
            head_pitch: 0.0,
            timestamp: Instant::now(),
        };
        update_fallback_debug_state(0.0, 0.0);
        set_latest_raw_gaze(sample);
    }
}

/// Boucle de repli portable et tolérante aux pannes
fn run_fallback_camera_loop() {
    while is_camera_worker_running() {
        let _ = check_and_stop_eye_tracking_on_escape();
        if is_eye_tracking_enabled() || is_calibration_active() || is_dev_mode() {
            poll_mouse_fallback_sample();
        }
        std::thread::sleep(Duration::from_millis(33));
    }
}