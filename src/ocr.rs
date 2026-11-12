use crate::automation::NumberedElementTarget;
#[cfg(windows)]
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use std::time::Duration;

#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
    GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HDC, SRCCOPY,
};
#[cfg(windows)]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
#[cfg(windows)]
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, WindowFromPoint,
};

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn PrintWindow(hwnd: HWND, hdc: HDC, nflags: u32) -> BOOL;
}

#[cfg(not(windows))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HWND(pub *mut std::ffi::c_void);

#[cfg(not(windows))]
unsafe impl Send for HWND {}
#[cfg(not(windows))]
unsafe impl Sync for HWND {}

#[cfg(not(windows))]
impl HWND {
    pub const NULL: Self = HWND(std::ptr::null_mut());

    pub fn is_invalid(&self) -> bool {
        self.0.is_null()
    }
}

#[cfg(windows)]
fn apply_window_occlusion_mask(target_hwnd: HWND, rect: [i32; 4], buffer: &mut [u8], w: u32, h: u32) {
    let current_pid = std::process::id();

    struct OcclusionContext {
        target_hwnd: HWND,
        current_pid: u32,
        capture_rect: [i32; 4],
        passed_target: bool,
        occluding_rects: Vec<[i32; 4]>,
    }

    unsafe extern "system" fn enum_occluding_windows(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut OcclusionContext);
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return BOOL(1);
        }

        let mut pid = 0u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let is_libertide = pid == ctx.current_pid;

        if hwnd == ctx.target_hwnd {
            ctx.passed_target = true;
            return BOOL(1);
        }

        let should_mask = is_libertide || !ctx.passed_target;
        if should_mask {
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r).is_ok() {
                let ox0 = r.left.max(ctx.capture_rect[0]);
                let oy0 = r.top.max(ctx.capture_rect[1]);
                let ox1 = r.right.min(ctx.capture_rect[2]);
                let oy1 = r.bottom.min(ctx.capture_rect[3]);
                if ox1 > ox0 && oy1 > oy0 {
                    ctx.occluding_rects.push([ox0, oy0, ox1, oy1]);
                }
            }
        }
        BOOL(1)
    }

    let mut ctx = OcclusionContext {
        target_hwnd,
        current_pid,
        capture_rect: rect,
        passed_target: false,
        occluding_rects: Vec::new(),
    };

    unsafe {
        let _ = EnumWindows(Some(enum_occluding_windows), LPARAM(&mut ctx as *mut _ as isize));
    }

    for o_rect in ctx.occluding_rects {
        let x0 = (o_rect[0] - rect[0]).clamp(0, w as i32) as usize;
        let y0 = (o_rect[1] - rect[1]).clamp(0, h as i32) as usize;
        let x1 = (o_rect[2] - rect[0]).clamp(0, w as i32) as usize;
        let y1 = (o_rect[3] - rect[1]).clamp(0, h as i32) as usize;

        for y in y0..y1 {
            let row_offset = y * (w as usize) * 4;
            for x in x0..x1 {
                let idx = row_offset + x * 4;
                buffer[idx..idx + 4].copy_from_slice(&[32, 32, 32, 255]);
            }
        }
    }
}

#[cfg(not(windows))]
impl Default for HWND {
    fn default() -> Self {
        Self::NULL
    }
}

/// Capture la zone d'une fenêtre en buffer RGBA 32 bits en mémoire (< 3 ms)
#[cfg(windows)]
pub fn capture_window_rgba(hwnd: HWND, rect: [i32; 4]) -> Option<(Vec<u8>, u32, u32)> {
    let w = (rect[2] - rect[0]).max(1) as u32;
    let h = (rect[3] - rect[1]).max(1) as u32;
    
    if w < 10 || h < 10 {
        return None;
    }

    unsafe {
        let hdc_screen = GetDC(HWND(std::ptr::null_mut()));
        if hdc_screen.0.is_null() {
            return None;
        }
        let hdc_mem = CreateCompatibleDC(hdc_screen);
        let hbm = CreateCompatibleBitmap(hdc_screen, w as i32, h as i32);
        let old_bm = SelectObject(hdc_mem, hbm);

        // 1. Essai de capture isolée via PrintWindow (PW_RENDERFULLCONTENT = 2)
        let mut isolated_success = false;
        if !hwnd.is_invalid() && !crate::window_util::is_window_hung(hwnd) {
            const PW_RENDERFULLCONTENT: u32 = 2;
            isolated_success = PrintWindow(hwnd, hdc_mem, PW_RENDERFULLCONTENT).as_bool();
        }

        // 2. Repli sur BitBlt de l'écran uniquement si la fenêtre est réellement visible
        let is_visible = hwnd.0.is_null() || crate::window_util::is_window_visible_on_screen(hwnd);
        let used_screen_fallback = !isolated_success && is_visible;

        if !isolated_success && !is_visible {
            // Aucune capture bitmap de la fenêtre n'a pu être faite (fenêtre invisible)
            SelectObject(hdc_mem, old_bm);
            let _ = DeleteObject(hbm);
            let _ = DeleteDC(hdc_mem);
            ReleaseDC(HWND(std::ptr::null_mut()), hdc_screen);
            return None;
        }

        if used_screen_fallback {
            let _ = BitBlt(
                hdc_mem, 0, 0, w as i32, h as i32,
                hdc_screen, rect[0], rect[1], SRCCOPY,
            );
        }

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // Top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut buffer = vec![0u8; (w * h * 4) as usize];
        let lines = GetDIBits(
            hdc_mem,
            hbm,
            0,
            h,
            Some(buffer.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        SelectObject(hdc_mem, old_bm);
        let _ = DeleteObject(hbm);
        let _ = DeleteDC(hdc_mem);

        // Détection de l'échec silencieux de PrintWindow (écran noir fréquent sous Chrome/Edge/DirectX)
        if isolated_success && lines > 0 && is_visible {
            let sample_step = ((w * h) / 100).max(1) as usize;
            let mut has_color = false;
            for i in (0..(w * h) as usize).step_by(sample_step) {
                let idx = i * 4;
                if idx + 2 < buffer.len() {
                    if buffer[idx] > 10 || buffer[idx + 1] > 10 || buffer[idx + 2] > 10 {
                        has_color = true;
                        break;
                    }
                }
            }

            // Si le rendu interne est tout noir, basculer sur le BitBlt direct de l'écran visible
            if !has_color {
                let hdc_mem_retry = CreateCompatibleDC(hdc_screen);
                let hbm_retry = CreateCompatibleBitmap(hdc_screen, w as i32, h as i32);
                let old_bm_retry = SelectObject(hdc_mem_retry, hbm_retry);
                let _ = BitBlt(hdc_mem_retry, 0, 0, w as i32, h as i32, hdc_screen, rect[0], rect[1], SRCCOPY);
                let _ = GetDIBits(hdc_mem_retry, hbm_retry, 0, h, Some(buffer.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
                SelectObject(hdc_mem_retry, old_bm_retry);
                let _ = DeleteObject(hbm_retry);
                let _ = DeleteDC(hdc_mem_retry);
                apply_window_occlusion_mask(hwnd, rect, &mut buffer, w, h);
            }
        }

        ReleaseDC(HWND(std::ptr::null_mut()), hdc_screen);

        if lines > 0 {
            if used_screen_fallback {
                apply_window_occlusion_mask(hwnd, rect, &mut buffer, w, h);
            }
            Some((buffer, w, h))
        } else {
            None
        }
    }
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[cfg(target_os = "macos")]
#[link(name = "Vision", kind = "framework")]
#[link(name = "CoreGraphics", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
#[link(name = "objc", kind = "dylib")]
extern "C" {
    fn objc_getClass(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn sel_registerName(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn objc_msgSend();
    fn CGWindowListCreateImage(
        screen_bounds: CGRect,
        list_options: u32,
        window_id: u32,
        image_options: u32,
    ) -> *mut std::ffi::c_void;
    fn CGImageGetWidth(image: *mut std::ffi::c_void) -> usize;
    fn CGImageGetHeight(image: *mut std::ffi::c_void) -> usize;
    fn CGImageGetBytesPerRow(image: *mut std::ffi::c_void) -> usize;
    fn CGImageGetDataProvider(image: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGDataProviderCopyData(provider: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CFDataGetBytePtr(data: *mut std::ffi::c_void) -> *const u8;
    fn CFDataGetLength(data: *mut std::ffi::c_void) -> isize;
    fn CFRelease(cf: *mut std::ffi::c_void);
}

/// Capture native sous macOS via CoreGraphics (< 4 ms)
#[cfg(target_os = "macos")]
pub fn capture_window_rgba(_hwnd: HWND, rect: [i32; 4]) -> Option<(Vec<u8>, u32, u32)> {
    let w = (rect[2] - rect[0]).max(1) as u32;
    let h = (rect[3] - rect[1]).max(1) as u32;

    let bounds = CGRect {
        origin: CGPoint {
            x: rect[0] as f64,
            y: rect[1] as f64,
        },
        size: CGSize {
            width: w as f64,
            height: h as f64,
        },
    };

    unsafe {
        // kCGWindowListOptionOnScreenOnly = 1, kCGNullWindowID = 0, kCGWindowImageDefault = 0
        let cg_image = CGWindowListCreateImage(bounds, 1, 0, 0);
        if cg_image.is_null() {
            return None;
        }

        let img_w = CGImageGetWidth(cg_image) as u32;
        let img_h = CGImageGetHeight(cg_image) as u32;
        let bpr = CGImageGetBytesPerRow(cg_image);

        let provider = CGImageGetDataProvider(cg_image);
        if provider.is_null() {
            CFRelease(cg_image);
            return None;
        }

        let data = CGDataProviderCopyData(provider);
        if data.is_null() {
            CFRelease(cg_image);
            return None;
        }

        let ptr = CFDataGetBytePtr(data);
        let total_len = CFDataGetLength(data) as usize;

        if ptr.is_null() || total_len == 0 || img_w == 0 || img_h == 0 {
            CFRelease(data);
            CFRelease(cg_image);
            return None;
        }

        let row_bytes = (img_w as usize) * 4;
        let mut buffer = vec![0u8; (img_w * img_h * 4) as usize];

        for y in 0..img_h as usize {
            let src_offset = y * bpr;
            if src_offset + row_bytes <= total_len {
                let src_slice = std::slice::from_raw_parts(ptr.add(src_offset), row_bytes);
                let dst_offset = y * row_bytes;
                buffer[dst_offset..dst_offset + row_bytes].copy_from_slice(src_slice);
            }
        }

        CFRelease(data);
        CFRelease(cg_image);

        Some((buffer, img_w, img_h))
    }
}

#[cfg(target_os = "macos")]
unsafe fn msg_send_0(obj: *mut std::ffi::c_void, sel: *mut std::ffi::c_void) -> *mut std::ffi::c_void {
    let f: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> *mut std::ffi::c_void =
        std::mem::transmute(objc_msgSend as *const ());
    f(obj, sel)
}

#[cfg(target_os = "macos")]
unsafe fn get_bounding_box(obs: *mut std::ffi::c_void) -> CGRect {
    let sel_bounding_box = sel_registerName(b"boundingBox\0".as_ptr() as *const _);
    #[cfg(target_arch = "aarch64")]
    {
        let msg_fn: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> CGRect =
            std::mem::transmute(objc_msgSend as *const ());
        msg_fn(obs, sel_bounding_box)
    }
    #[cfg(target_arch = "x86_64")]
    {
        extern "C" {
            fn objc_msgSend_stret(stret_addr: *mut CGRect, obj: *mut std::ffi::c_void, sel: *mut std::ffi::c_void);
        }
        let mut r = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: 0.0, height: 0.0 },
        };
        objc_msgSend_stret(&mut r, obs, sel_bounding_box);
        r
    }
}

/// Reconnaissance de texte native macOS via Apple Vision (VNRecognizeTextRequest)
/// Accélérée par l'Apple Neural Engine (ANE) sur puces M1/M2/M3/M4 (< 5 ms)
#[cfg(target_os = "macos")]
pub fn recognize_texts_vision(rect: [i32; 4]) -> Vec<(String, [i32; 4])> {
    let w = (rect[2] - rect[0]).max(1) as f64;
    let h = (rect[3] - rect[1]).max(1) as f64;

    let bounds = CGRect {
        origin: CGPoint { x: rect[0] as f64, y: rect[1] as f64 },
        size: CGSize { width: w, height: h },
    };

    unsafe {
        let cg_image = CGWindowListCreateImage(bounds, 1, 0, 0);
        if cg_image.is_null() {
            return Vec::new();
        }

        let cls_pool = objc_getClass(b"NSAutoreleasePool\0".as_ptr() as *const _);
        let sel_alloc = sel_registerName(b"alloc\0".as_ptr() as *const _);
        let sel_init = sel_registerName(b"init\0".as_ptr() as *const _);
        let sel_release = sel_registerName(b"release\0".as_ptr() as *const _);
        let pool = if !cls_pool.is_null() {
            msg_send_0(msg_send_0(cls_pool, sel_alloc), sel_init)
        } else {
            std::ptr::null_mut()
        };

        let cls_req = objc_getClass(b"VNRecognizeTextRequest\0".as_ptr() as *const _);
        let cls_handler = objc_getClass(b"VNImageRequestHandler\0".as_ptr() as *const _);
        let cls_array = objc_getClass(b"NSArray\0".as_ptr() as *const _);

        let mut results = Vec::new();

        if !cls_req.is_null() && !cls_handler.is_null() && !cls_array.is_null() {
            let req = msg_send_0(msg_send_0(cls_req, sel_alloc), sel_init);

            // Niveau de reconnaissance précis (0 = accurate) pour capturer l'ensemble des textes d'une même police
            let sel_set_level = sel_registerName(b"setRecognitionLevel:\0".as_ptr() as *const _);
            let set_level_fn: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, isize) =
                std::mem::transmute(objc_msgSend as *const ());
            set_level_fn(req, sel_set_level, 0);

            // Hauteur minimale ramenée à 0.0 pour détecter les plus petits caractères et indices
            let sel_set_min_height = sel_registerName(b"setMinimumTextHeight:\0".as_ptr() as *const _);
            let set_min_height_fn: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, f32) =
                std::mem::transmute(objc_msgSend as *const ());
            set_min_height_fn(req, sel_set_min_height, 0.0);

            let sel_init_with_cg = sel_registerName(b"initWithCGImage:options:\0".as_ptr() as *const _);
            let init_cg_fn: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, *mut std::ffi::c_void, *mut std::ffi::c_void) -> *mut std::ffi::c_void =
                std::mem::transmute(objc_msgSend as *const ());
            let handler = init_cg_fn(msg_send_0(cls_handler, sel_alloc), sel_init_with_cg, cg_image, std::ptr::null_mut());

            let sel_array_with_obj = sel_registerName(b"arrayWithObject:\0".as_ptr() as *const _);
            let array_fn: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, *mut std::ffi::c_void) -> *mut std::ffi::c_void =
                std::mem::transmute(objc_msgSend as *const ());
            let req_array = array_fn(cls_array, sel_array_with_obj, req);

            let sel_perform = sel_registerName(b"performRequests:error:\0".as_ptr() as *const _);
            let perform_fn: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, *mut std::ffi::c_void, *mut *mut std::ffi::c_void) -> u8 =
                std::mem::transmute(objc_msgSend as *const ());
            let mut err: *mut std::ffi::c_void = std::ptr::null_mut();
            let ok = perform_fn(handler, sel_perform, req_array, &mut err);

            if ok != 0 {
                let sel_results = sel_registerName(b"results\0".as_ptr() as *const _);
                let obs_array = msg_send_0(req, sel_results);
                if !obs_array.is_null() {
                    let sel_count = sel_registerName(b"count\0".as_ptr() as *const _);
                    let count_fn: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> usize =
                        std::mem::transmute(objc_msgSend as *const ());
                    let sel_obj_at = sel_registerName(b"objectAtIndex:\0".as_ptr() as *const _);
                    let obj_at_fn: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, usize) -> *mut std::ffi::c_void =
                        std::mem::transmute(objc_msgSend as *const ());
                    let sel_top = sel_registerName(b"topCandidates:\0".as_ptr() as *const _);
                    let sel_str = sel_registerName(b"string\0".as_ptr() as *const _);
                    let sel_utf8 = sel_registerName(b"UTF8String\0".as_ptr() as *const _);
                    let utf8_fn: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> *const std::ffi::c_char =
                        std::mem::transmute(objc_msgSend as *const ());

                    let count = count_fn(obs_array, sel_count).min(350);
                    for i in 0..count {
                        let obs = obj_at_fn(obs_array, sel_obj_at, i);
                        let cands = obj_at_fn(obs, sel_top, 1);
                        if count_fn(cands, sel_count) > 0 {
                            let top = obj_at_fn(cands, sel_obj_at, 0);
                            let ns_txt = msg_send_0(top, sel_str);
                            let c_txt = utf8_fn(ns_txt, sel_utf8);
                            if !c_txt.is_null() {
                                let txt = std::ffi::CStr::from_ptr(c_txt).to_string_lossy().trim().to_string();
                                if !txt.is_empty() {
                                    let b = get_bounding_box(obs);
                                    let bx0 = rect[0] + (b.origin.x * w).round() as i32;
                                    let by0 = rect[1] + ((1.0 - b.origin.y - b.size.height) * h).round() as i32;
                                    let bw = (b.size.width * w).round() as i32;
                                    let bh = (b.size.height * h).round() as i32;
                                    results.push((txt, [bx0, by0, bx0 + bw, by0 + bh]));
                                }
                            }
                        }
                    }
                }
            }
            msg_send_0(handler, sel_release);
            msg_send_0(req, sel_release);
        }

        if !pool.is_null() {
            msg_send_0(pool, sel_release);
        }
        CFRelease(cg_image);
        results
    }
}

#[cfg(target_os = "linux")]
fn get_ocr_engine() -> Option<&'static ocrs::OcrEngine> {
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::OnceLock;
    use ocrs::{OcrEngine, OcrEngineParams};
    use rten::Model;

    static ENGINE: OnceLock<Option<OcrEngine>> = OnceLock::new();

    ENGINE.get_or_init(|| {
        fn ensure_model(name: &str) -> Option<PathBuf> {
            let mut candidates = vec![
                PathBuf::from(name),
                PathBuf::from("models").join(name),
                PathBuf::from("/usr/share/libertide/models").join(name),
            ];
            if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
                let base = PathBuf::from(dir);
                candidates.push(base.join("models").join(name));
                candidates.push(base.join(name));
            }
            if let Ok(home) = std::env::var("HOME") {
                candidates.push(PathBuf::from(home).join(".cache/libertide/models").join(name));
            }

            if let Some(existing) = candidates.into_iter().find(|p| p.is_file()) {
                return Some(existing);
            }

            // Téléchargement automatique vers le cache si inexistant
            let target_dir = std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".cache/libertide/models"))
                .unwrap_or_else(|_| PathBuf::from("models"));

            let _ = std::fs::create_dir_all(&target_dir);
            let target_path = target_dir.join(name);
            let temp_path = target_dir.join(format!("{name}.tmp"));

            let url = format!("https://robertknight.github.io/ocrs-models/{name}");
            let ok = Command::new("curl")
                .args(["-f", "-L", "-sS", "-o"])
                .arg(&temp_path)
                .arg(&url)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
                || Command::new("wget")
                    .args(["-q", "-O"])
                    .arg(&temp_path)
                    .arg(&url)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);

            if ok && temp_path.is_file() {
                if std::fs::rename(&temp_path, &target_path).is_ok() {
                    return Some(target_path);
                }
            }
            let _ = std::fs::remove_file(&temp_path);
            None
        }

        let det_path = ensure_model("text-detection.rten")
            .or_else(|| ensure_model("text-detection.onnx"))?;
        let rec_path = ensure_model("text-recognition.rten")
            .or_else(|| ensure_model("text-recognition.onnx"))?;

        let detection_model = rten::Model::load_file(&det_path).ok()?;
        let recognition_model = rten::Model::load_file(&rec_path).ok()?;

        OcrEngine::new(OcrEngineParams {
            detection_model: Some(detection_model),
            recognition_model: Some(recognition_model),
            ..Default::default()
        }).ok()
    }).as_ref()
}

/// OCR haute performance pur Rust sous Linux via ocrs + rten
#[cfg(target_os = "linux")]
pub fn recognize_texts_ocrs(
    pixels: &[u8],
    w: u32,
    h: u32,
    window_rect: [i32; 4],
) -> Vec<(String, [i32; 4])> {
    if w < 10 || h < 10 {
        return Vec::new();
    }

    let Some(engine) = get_ocr_engine() else {
        return Vec::new();
    };

    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for chunk in pixels.chunks_exact(4) {
        rgb.push(chunk[2]);
        rgb.push(chunk[1]);
        rgb.push(chunk[0]);
    }

    let Ok(img_source) = ocrs::ImageSource::from_bytes(&rgb, (w, h)) else {
        return Vec::new();
    };
    let Ok(input) = engine.prepare_input(img_source) else {
        return Vec::new();
    };
    let Ok(word_rects) = engine.detect_words(&input) else {
        return Vec::new();
    };
    let line_rects = engine.find_text_lines(&input, &word_rects);
    let Ok(texts) = engine.recognize_text(&input, &line_rects) else {
        return Vec::new();
    };

    let scale_x = if w > 0 { (window_rect[2] - window_rect[0]).max(1) as f32 / w as f32 } else { 1.0 };
    let scale_y = if h > 0 { (window_rect[3] - window_rect[1]).max(1) as f32 / h as f32 } else { 1.0 };

    let mut out = Vec::new();
    for (line, text_opt) in line_rects.iter().zip(texts.into_iter()) {
        if let Some(txt) = text_opt {
            let s = txt.to_string().trim().to_string();
            if !s.is_empty() {
                let mut min_x = i32::MAX;
                let mut min_y = i32::MAX;
                let mut max_x = i32::MIN;
                let mut max_y = i32::MIN;
                for word in line {
                    for pt in word.corners() {
                        min_x = min_x.min(pt.x.floor() as i32);
                        min_y = min_y.min(pt.y.floor() as i32);
                        max_x = max_x.max(pt.x.ceil() as i32);
                        max_y = max_y.max(pt.y.ceil() as i32);
                    }
                }
                if min_x < max_x && min_y < max_y {
                    let x0 = window_rect[0] + (min_x as f32 * scale_x).round() as i32;
                    let y0 = window_rect[1] + (min_y as f32 * scale_y).round() as i32;
                    let x1 = window_rect[0] + (max_x as f32 * scale_x).round() as i32;
                    let y1 = window_rect[1] + (max_y as f32 * scale_y).round() as i32;
                    out.push((s, [x0, y0, x1, y1]));
                }
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct XImageFuncs {
    create_image: *const (),
    destroy_image: unsafe extern "C" fn(*mut XImage) -> std::ffi::c_int,
    get_pixel: *const (),
    put_pixel: *const (),
    sub_image: *const (),
    add_pixel: *const (),
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct XImage {
    width: i32,
    height: i32,
    xoffset: i32,
    format: i32,
    data: *mut u8,
    byte_order: i32,
    bitmap_unit: i32,
    bitmap_bit_order: i32,
    bitmap_pad: i32,
    depth: i32,
    bytes_per_line: i32,
    bits_per_pixel: i32,
    red_mask: usize,
    green_mask: usize,
    blue_mask: usize,
    obdata: *mut std::ffi::c_void,
    f: XImageFuncs,
}

#[cfg(target_os = "linux")]
extern "C" {
    fn dlopen(filename: *const std::ffi::c_char, flag: std::ffi::c_int) -> *mut std::ffi::c_void;
    fn dlsym(handle: *mut std::ffi::c_void, symbol: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn dlclose(handle: *mut std::ffi::c_void) -> std::ffi::c_int;
}

/// Décodage du format PPM P6 (binaire RGB) en tampon BGRA 32 bits (< 2 ms)
#[cfg(target_os = "linux")]
fn parse_ppm_p6(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if data.len() < 10 || &data[0..2] != b"P6" {
        return None;
    }
    let mut idx = 2;
    let mut tokens = Vec::with_capacity(3);

    while tokens.len() < 3 && idx < data.len() {
        while idx < data.len() {
            if data[idx] == b'#' {
                while idx < data.len() && data[idx] != b'\n' {
                    idx += 1;
                }
            } else if data[idx].is_ascii_whitespace() {
                idx += 1;
            } else {
                break;
            }
        }
        if idx >= data.len() {
            break;
        }
        let start = idx;
        while idx < data.len() && !data[idx].is_ascii_whitespace() && data[idx] != b'#' {
            idx += 1;
        }
        let token_str = std::str::from_utf8(&data[start..idx]).ok()?;
        let num: u32 = token_str.parse().ok()?;
        tokens.push(num);
    }

    if tokens.len() != 3 {
        return None;
    }

    let w = tokens[0];
    let h = tokens[1];
    let max_val = tokens[2];
    if w == 0 || h == 0 || max_val == 0 {
        return None;
    }

    if idx < data.len() && data[idx].is_ascii_whitespace() {
        idx += 1;
    }

    let payload = &data[idx..];
    let is_16bit = max_val > 255;
    let bytes_per_pixel = if is_16bit { 6 } else { 3 };
    let expected_len = (w as usize) * (h as usize) * bytes_per_pixel;

    if payload.len() < expected_len {
        return None;
    }

    let mut buffer = vec![0u8; (w * h * 4) as usize];
    if !is_16bit && max_val == 255 {
        for i in 0..(w * h) as usize {
            let src = i * 3;
            let dst = i * 4;
            buffer[dst] = payload[src + 2];     // B
            buffer[dst + 1] = payload[src + 1]; // G
            buffer[dst + 2] = payload[src];     // R
            buffer[dst + 3] = 255;              // A
        }
    } else if !is_16bit {
        for i in 0..(w * h) as usize {
            let src = i * 3;
            let dst = i * 4;
            buffer[dst] = ((payload[src + 2] as u32 * 255) / max_val) as u8;
            buffer[dst + 1] = ((payload[src + 1] as u32 * 255) / max_val) as u8;
            buffer[dst + 2] = ((payload[src] as u32 * 255) / max_val) as u8;
            buffer[dst + 3] = 255;
        }
    } else {
        for i in 0..(w * h) as usize {
            let src = i * 6;
            let dst = i * 4;
            buffer[dst] = (u16::from_be_bytes([payload[src + 4], payload[src + 5]]) as u32 * 255 / max_val) as u8;
            buffer[dst + 1] = (u16::from_be_bytes([payload[src + 2], payload[src + 3]]) as u32 * 255 / max_val) as u8;
            buffer[dst + 2] = (u16::from_be_bytes([payload[src], payload[src + 1]]) as u32 * 255 / max_val) as u8;
            buffer[dst + 3] = 255;
        }
    }

    Some((buffer, w, h))
}

/// Capture native sous Linux Wayland via `grim` (< 8 ms)
#[cfg(target_os = "linux")]
fn capture_window_wayland(rect: [i32; 4]) -> Option<(Vec<u8>, u32, u32)> {
    let w = (rect[2] - rect[0]).max(1);
    let h = (rect[3] - rect[1]).max(1);
    let geom = format!("{},{} {}x{}", rect[0].max(0), rect[1].max(0), w, h);

    let output = std::process::Command::new("grim")
        .args(["-g", &geom, "-t", "ppm", "-"])
        .output()
        .ok()?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    parse_ppm_p6(&output.stdout)
}

/// Capture native sous Linux via X11 / XGetImage (< 4 ms)
#[cfg(target_os = "linux")]
fn capture_window_x11(_hwnd: HWND, rect: [i32; 4]) -> Option<(Vec<u8>, u32, u32)> {
    unsafe {
        const RTLD_LAZY: std::ffi::c_int = 1;

        let lib_name = b"libX11.so.6\0";
        let mut handle = dlopen(lib_name.as_ptr() as *const std::ffi::c_char, RTLD_LAZY);
        if handle.is_null() {
            let alt_name = b"libX11.so\0";
            handle = dlopen(alt_name.as_ptr() as *const std::ffi::c_char, RTLD_LAZY);
        }

        if handle.is_null() {
            return None;
        }

        type XOpenDisplayFn = unsafe extern "C" fn(*const std::ffi::c_char) -> *mut std::ffi::c_void;
        type XCloseDisplayFn = unsafe extern "C" fn(*mut std::ffi::c_void) -> std::ffi::c_int;
        type XDefaultRootWindowFn = unsafe extern "C" fn(*mut std::ffi::c_void) -> usize;
        type XGetImageFn = unsafe extern "C" fn(
            *mut std::ffi::c_void,
            usize,
            i32,
            i32,
            u32,
            u32,
            usize,
            i32,
        ) -> *mut XImage;

        let sym_open = dlsym(handle, b"XOpenDisplay\0".as_ptr() as *const std::ffi::c_char);
        let sym_close = dlsym(handle, b"XCloseDisplay\0".as_ptr() as *const std::ffi::c_char);
        let sym_root = dlsym(handle, b"XDefaultRootWindow\0".as_ptr() as *const std::ffi::c_char);
        let sym_get = dlsym(handle, b"XGetImage\0".as_ptr() as *const std::ffi::c_char);

        if sym_open.is_null() || sym_close.is_null() || sym_root.is_null() || sym_get.is_null() {
            dlclose(handle);
            return None;
        }

        let x_open_display: XOpenDisplayFn = std::mem::transmute(sym_open);
        let x_close_display: XCloseDisplayFn = std::mem::transmute(sym_close);
        let x_default_root: XDefaultRootWindowFn = std::mem::transmute(sym_root);
        let x_get_image: XGetImageFn = std::mem::transmute(sym_get);

        let dpy = x_open_display(std::ptr::null());
        if dpy.is_null() {
            dlclose(handle);
            return None;
        }

        let root = x_default_root(dpy);
        let w = (rect[2] - rect[0]).max(1) as u32;
        let h = (rect[3] - rect[1]).max(1) as u32;

        // AllPlanes = !0, ZPixmap = 2
        let ximage = x_get_image(dpy, root, rect[0].max(0), rect[1].max(0), w, h, !0, 2);
        if ximage.is_null() {
            x_close_display(dpy);
            dlclose(handle);
            return None;
        }

        let img = &*ximage;
        let img_w = img.width.max(1) as u32;
        let img_h = img.height.max(1) as u32;
        let bpl = img.bytes_per_line.max(1) as usize;
        let data_ptr = img.data;

        let mut buffer = vec![0u8; (img_w * img_h * 4) as usize];

        if !data_ptr.is_null() {
            if img.bits_per_pixel == 32 {
                for y in 0..img_h as usize {
                    let src_row = std::slice::from_raw_parts(
                        data_ptr.add(y * bpl),
                        (img_w * 4) as usize,
                    );
                    let dst_offset = y * (img_w as usize * 4);
                    buffer[dst_offset..dst_offset + (img_w as usize * 4)].copy_from_slice(src_row);
                }
            } else if img.bits_per_pixel == 24 {
                for y in 0..img_h as usize {
                    for x in 0..img_w as usize {
                        let src_idx = y * bpl + x * 3;
                        let dst_idx = (y * img_w as usize + x) * 4;
                        let b = *data_ptr.add(src_idx);
                        let g = *data_ptr.add(src_idx + 1);
                        let r = *data_ptr.add(src_idx + 2);
                        buffer[dst_idx] = b;
                        buffer[dst_idx + 1] = g;
                        buffer[dst_idx + 2] = r;
                        buffer[dst_idx + 3] = 255;
                    }
                }
            }
        }

        ((img.f.destroy_image))(ximage);
        x_close_display(dpy);
        dlclose(handle);

        Some((buffer, img_w, img_h))
    }
}

/// Capture sous Linux avec bascule intelligente Wayland (grim) / X11 (XGetImage)
#[cfg(target_os = "linux")]
pub fn capture_window_rgba(hwnd: HWND, rect: [i32; 4]) -> Option<(Vec<u8>, u32, u32)> {
    let is_wayland = std::env::var("WAYLAND_DISPLAY").is_ok()
        || std::env::var("XDG_SESSION_TYPE")
            .map(|s| s.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false);

    if is_wayland {
        if let Some(res) = capture_window_wayland(rect) {
            return Some(res);
        }
        capture_window_x11(hwnd, rect)
    } else {
        if let Some(res) = capture_window_x11(hwnd, rect) {
            return Some(res);
        }
        capture_window_wayland(rect)
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn capture_window_rgba(_hwnd: HWND, _rect: [i32; 4]) -> Option<(Vec<u8>, u32, u32)> {
    None
}

#[cfg(windows)]
fn resolve_visual_zone_content(
    hwnd: HWND,
    rect: [i32; 4],
    click_x: i32,
    click_y: i32,
) -> Option<String> {
    let current_pid = std::process::id();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
            let mid_x = click_x;
            let mid_y = click_y;
            let dx = ((rect[2] - rect[0]) / 4).max(2);
            let dy = ((rect[3] - rect[1]) / 4).max(2);
            let sample_points = [
                POINT { x: mid_x, y: mid_y },
                POINT { x: (rect[0] + dx).min(rect[2]), y: mid_y },
                POINT { x: (rect[2] - dx).max(rect[0]), y: mid_y },
                POINT { x: mid_x, y: (rect[1] + dy).min(rect[3]) },
            ];

            for pt in sample_points {
                if let Ok(elem) = uia.ElementFromPoint(pt) {
                    if let Ok(elem_pid) = elem.CurrentProcessId() {
                        if elem_pid as u32 == current_pid {
                            continue;
                        }
                    }
                    if let Ok(name) = elem.CurrentName() {
                        let s = name.to_string();
                        let trimmed = s.trim();
                        let lower = trimmed.to_lowercase();
                        if !trimmed.is_empty()
                            && lower != "pane"
                            && lower != "group"
                            && lower != "groupe"
                            && lower != "window"
                            && lower != "client"
                            && lower != "panneau"
                        {
                            return Some(trimmed.to_string());
                        }
                    }

                    if let Some(txt) = crate::ui::extract_inner_text(&uia, &elem, 300) {
                        let trimmed = txt.trim();
                        let lower = trimmed.to_lowercase();
                        if !trimmed.is_empty()
                            && lower != "pane"
                            && lower != "group"
                            && lower != "groupe"
                            && lower != "window"
                            && lower != "client"
                            && lower != "panneau"
                        {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
        }

        let pt = POINT { x: click_x, y: click_y };
        let child = WindowFromPoint(pt);
        let mut child_pid = 0u32;
        let _ = GetWindowThreadProcessId(child, Some(&mut child_pid));
        if !child.is_invalid() && child != hwnd && child_pid != current_pid {
            let mut buf = [0u16; 512];
            let len = GetWindowTextW(child, &mut buf);
            if len > 0 {
                let s = String::from_utf16_lossy(&buf[..len as usize]);
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn resolve_visual_zone_content(
    _hwnd: HWND,
    rect: [i32; 4],
    _click_x: i32,
    _click_y: i32,
) -> Option<String> {
    recognize_texts_vision(rect).into_iter().next().map(|(txt, _)| txt)
}

#[cfg(target_os = "linux")]
fn resolve_visual_zone_content(
    _hwnd: HWND,
    rect: [i32; 4],
    _click_x: i32,
    _click_y: i32,
) -> Option<String> {
    let (pixels, w, h) = capture_window_rgba(HWND::NULL, rect)?;
    recognize_texts_ocrs(&pixels, w, h, rect).into_iter().next().map(|(txt, _)| txt)
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn resolve_visual_zone_content(
    _hwnd: HWND,
    _rect: [i32; 4],
    _click_x: i32,
    _click_y: i32,
) -> Option<String> {
    None
}

/// Analyse purement logicielle des contrastes de contours et extraction des boîtes candidates
pub fn detect_visual_boxes_from_pixels(
    pixels: &[u8],
    w: u32,
    h: u32,
    window_rect: [i32; 4],
    existing_targets: &[NumberedElementTarget],
) -> Vec<[i32; 4]> {
    if w < 10 || h < 10 {
        return Vec::new();
    }

    let required_len = (w as usize) * (h as usize) * 4;
    if pixels.len() < required_len || w < 8 || h < 6 {
        return Vec::new();
    }

    // Découpage en micro-cellules (6x5) pour détecter les plus petits caractères (lettres fines, exposants, ponctuation)
    const CELL_W: u32 = 6;
    const CELL_H: u32 = 5;

    let grid_cols = (w / CELL_W) as usize;
    let grid_rows = (h / CELL_H) as usize;
    if grid_cols == 0 || grid_rows == 0 {
        return Vec::new();
    }

    let scale_x = if w > 0 { (window_rect[2] - window_rect[0]).max(1) as f32 / w as f32 } else { 1.0 };
    let scale_y = if h > 0 { (window_rect[3] - window_rect[1]).max(1) as f32 / h as f32 } else { 1.0 };

    let mut grid = vec![false; grid_cols * grid_rows];

    // Échantillonnage bi-directionnel complet pixel par pixel pour ne pas rater les traits de 1 pixel
    for gy in 0..grid_rows {
        for gx in 0..grid_cols {
            let x_start = (gx as u32) * CELL_W;
            let y_start = (gy as u32) * CELL_H;
            let mut edge_count = 0;
            let mut samples = 0;

            for dy in 0..CELL_H {
                let py = y_start + dy;
                if py + 1 >= h { break; }
                let row_offset = (py * w * 4) as usize;
                let next_row_offset = ((py + 1) * w * 4) as usize;

                for dx in 0..CELL_W {
                    let px = x_start + dx;
                    if px + 1 >= w { break; }
                    let idx1 = row_offset + (px as usize) * 4;
                    let idx_r = row_offset + ((px + 1) as usize) * 4;
                    let idx_b = next_row_offset + (px as usize) * 4;

                    let b1 = pixels[idx1] as i32;
                    let g1 = pixels[idx1 + 1] as i32;
                    let r1 = pixels[idx1 + 2] as i32;
                    let lum1 = (r1 * 299 + g1 * 587 + b1 * 114) >> 10;

                    let b_r = pixels[idx_r] as i32;
                    let g_r = pixels[idx_r + 1] as i32;
                    let r_r = pixels[idx_r + 2] as i32;
                    let lum_r = (r_r * 299 + g_r * 587 + b_r * 114) >> 10;

                    let b_b = pixels[idx_b] as i32;
                    let g_b = pixels[idx_b + 1] as i32;
                    let r_b = pixels[idx_b + 2] as i32;
                    let lum_b = (r_b * 299 + g_b * 587 + b_b * 114) >> 10;

                    // Seuil abaissé à 14 pour capter l'anti-aliasing subpixel et les polices très fines
                    if (lum1 - lum_r).abs() > 14 || (lum1 - lum_b).abs() > 14 {
                        edge_count += 1;
                    }
                    samples += 1;
                }
            }

            // Tout bloc présentant au moins une transition de contraste est conservé (suppression du plafond de rejet des polices denses/grasses)
            if samples > 0 && edge_count >= 1 {
                grid[gy * grid_cols + gx] = true;
            }
        }
    }

    // Extraction des boîtes englobantes par composantes connexes avec pontage horizontal (mots d'une même ligne)
    let mut visited = vec![false; grid.len()];
    let mut candidate_boxes: Vec<[i32; 4]> = Vec::new();

    for gy in 0..grid_rows {
        for gx in 0..grid_cols {
            let idx = gy * grid_cols + gx;
            if !grid[idx] || visited[idx] {
                continue;
            }

            let mut min_gx = gx;
            let mut max_gx = gx;
            let mut min_gy = gy;
            let mut max_gy = gy;

            let mut queue = vec![(gx, gy)];
            visited[idx] = true;

            while let Some((cx, cy)) = queue.pop() {
                min_gx = min_gx.min(cx);
                max_gx = max_gx.max(cx);
                min_gy = min_gy.min(cy);
                max_gy = max_gy.max(cy);

                // Exploration des voisins immédiats et pontage horizontal (+/- 4 micro-cellules pour lier les caractères d'un même mot)
                for dy in [-1isize, 0, 1] {
                    let ny = cy as isize + dy;
                    if ny < 0 || ny >= grid_rows as isize { continue; }
                    for dx in [-4isize, -3, -2, -1, 0, 1, 2, 3, 4] {
                        let nx = cx as isize + dx;
                        if nx < 0 || nx >= grid_cols as isize { continue; }
                        let n_idx = (ny as usize) * grid_cols + (nx as usize);
                        if grid[n_idx] && !visited[n_idx] {
                            visited[n_idx] = true;
                            queue.push((nx as usize, ny as usize));
                        }
                    }
                }
            }

            let box_w = (max_gx - min_gx + 1) as u32 * CELL_W;
            let box_h = (max_gy - min_gy + 1) as u32 * CELL_H;

            // Conserver tous les textes, y compris les caractères individuels isolés (dès 6x5 pixels)
            if box_w >= 6 && box_h >= 5 && box_h <= 400 {
                let screen_x0 = window_rect[0] + ((min_gx as u32 * CELL_W) as f32 * scale_x).round() as i32;
                let screen_y0 = window_rect[1] + ((min_gy as u32 * CELL_H) as f32 * scale_y).round() as i32;
                let screen_x1 = screen_x0 + (box_w as f32 * scale_x).round() as i32;
                let screen_y1 = screen_y0 + (box_h as f32 * scale_y).round() as i32;

                // Rejeter uniquement si un contrôle UIA terminal couvre plus de 35% de la surface
                let overlaps_uia = existing_targets.iter().any(|t| {
                    let is_container = t.role == "fenêtre"
                        || t.role == "window"
                        || t.role == "pane"
                        || t.role == "group"
                        || t.role == "groupe"
                        || t.role == "client"
                        || t.role == "panneau";
                    if is_container {
                        return false;
                    }
                    // Si l'élément UIA n'a pas de nom textuel lisible, on ne supprime pas l'OCR
                    if t.name.trim().is_empty() {
                        return false;
                    }

                    let inter_x0 = screen_x0.max(t.rect[0]);
                    let inter_y0 = screen_y0.max(t.rect[1]);
                    let inter_x1 = screen_x1.min(t.rect[2]);
                    let inter_y1 = screen_y1.min(t.rect[3]);

                    if inter_x1 > inter_x0 && inter_y1 > inter_y0 {
                        let inter_area = ((inter_x1 - inter_x0) * (inter_y1 - inter_y0)) as f64;
                        let cand_area = (box_w * box_h) as f64;
                        inter_area / cand_area > 0.50
                    } else {
                        false
                    }
                });

                if !overlaps_uia {
                    candidate_boxes.push([screen_x0, screen_y0, screen_x1, screen_y1]);
                }
            }
        }
    }

    // Trier les cibles par ordre visuel de lecture (haut-vers-bas, gauche-vers-droite)
    candidate_boxes.sort_by_key(|b| (b[1] / 16) * 10000 + b[0]);
    candidate_boxes.truncate(400);

    candidate_boxes
}

/// Encode une image RGBA en flux mémoire BMP 24 bits (sans dépendance externe)
pub fn rgba_to_bmp(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let row_size = ((24 * width + 31) / 32) * 4;
    let image_size = row_size * height;
    let file_size = 54 + image_size;
    let mut bmp = Vec::with_capacity(file_size as usize);

    // BITMAPFILEHEADER (14 octets)
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&file_size.to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]); // Réservé
    bmp.extend_from_slice(&54u32.to_le_bytes()); // Décalage vers les pixels

    // BITMAPINFOHEADER (40 octets)
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(width as i32).to_le_bytes());
    bmp.extend_from_slice(&(-(height as i32)).to_le_bytes()); // Négatif = top-down
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&24u16.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    bmp.extend_from_slice(&image_size.to_le_bytes());
    bmp.extend_from_slice(&2835u32.to_le_bytes());
    bmp.extend_from_slice(&2835u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());

    let padding_bytes = (row_size - width * 3) as usize;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let idx = (y * (width as usize) + x) * 4;
            if idx + 2 < pixels.len() {
                bmp.push(pixels[idx]);     // B
                bmp.push(pixels[idx + 1]); // G
                bmp.push(pixels[idx + 2]); // R
            } else {
                bmp.extend_from_slice(&[0, 0, 0]);
            }
        }
        for _ in 0..padding_bytes {
            bmp.push(0);
        }
    }
    bmp
}

/// Encodage Base64 pur Rust sans crate supplémentaire
pub fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[derive(serde::Deserialize, Debug)]
struct VlmElement {
    #[serde(default)]
    name: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    box_2d: Option<[f32; 4]>,
    #[serde(default)]
    point: Option<[f32; 2]>,
}

/// Exécute le GUI Grounding visuel via Qwen2-VL pour découvrir les icônes muettes et composants canvas
pub fn detect_vlm_grounding_targets(
    pixels: &[u8],
    w: u32,
    h: u32,
    window_rect: [i32; 4],
    existing_targets: &[NumberedElementTarget],
) -> Vec<NumberedElementTarget> {
    if w < 20 || h < 20 || pixels.is_empty() {
        return Vec::new();
    }

    let max_dim = 960u32;
    let (target_w, target_h) = if w > max_dim || h > max_dim {
        let scale = (max_dim as f32 / w.max(h) as f32).min(1.0);
        ((w as f32 * scale).round() as u32, (h as f32 * scale).round() as u32)
    } else {
        (w, h)
    };

    let bmp_bytes = if target_w != w || target_h != h {
        let mut scaled = vec![0u8; (target_w * target_h * 4) as usize];
        for dy in 0..target_h {
            let sy = (dy * h / target_h) as usize;
            for dx in 0..target_w {
                let sx = (dx * w / target_w) as usize;
                let s_idx = (sy * w as usize + sx) * 4;
                let d_idx = (dy as usize * target_w as usize + dx as usize) * 4;
                if s_idx + 4 <= pixels.len() && d_idx + 4 <= scaled.len() {
                    scaled[d_idx..d_idx + 4].copy_from_slice(&pixels[s_idx..s_idx + 4]);
                }
            }
        }
        rgba_to_bmp(&scaled, target_w, target_h)
    } else {
        rgba_to_bmp(pixels, w, h)
    };

    let prompt = "Detect interactive icons, buttons, menus, canvas controls and cards lacking text. Output JSON list: [{\"name\": \"...\", \"role\": \"bouton\", \"box_2d\": [ymin, xmin, ymax, xmax]}] where coords are normalized 0..1000.";

    // 1. Tenter en priorité absolue le modèle Qwen-VL chargé localement en asynchrone
    if crate::local_llm::is_vlm_ready() {
        if let Some(raw_text) = crate::local_llm::query_vlm_grounding_local(prompt) {
            if let (Some(s), Some(e)) = (raw_text.find('['), raw_text.rfind(']')) {
                let clean = &raw_text[s..=e];
                if let Ok(elements) = serde_json::from_str::<Vec<VlmElement>>(clean) {
                    return convert_vlm_elements_to_targets(elements, window_rect, existing_targets);
                }
            }
        }
    }

    // 2. Repli vers le serveur / endpoint d'inférence configuré
    let b64 = base64_encode(&bmp_bytes);
    let data_uri = format!("data:image/bmp;base64,{b64}");

    let endpoint = std::env::var("QWENVL_ENDPOINT")
        .ok()
        .or_else(|| crate::fs_util::read_env_var_from_file(".env", "QWENVL_ENDPOINT"))
        .unwrap_or_else(|| "http://127.0.0.1:8000/v1/chat/completions".to_string());

    let model_name = std::env::var("QWENVL_MODEL")
        .ok()
        .or_else(|| crate::fs_util::read_env_var_from_file(".env", "QWENVL_MODEL"))
        .unwrap_or_else(|| "Qwen/Qwen2.5-VL-3B-Instruct".to_string());

    let api_key = std::env::var("QWENVL_API_KEY")
        .ok()
        .or_else(|| crate::fs_util::read_env_var_from_file(".env", "QWENVL_API_KEY"))
        .unwrap_or_default();

    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return Vec::new();
    };

    let client = match reqwest::Client::builder().timeout(Duration::from_millis(3500)).build() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let body = serde_json::json!({
        "model": model_name,
        "messages": [
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": prompt },
                    { "type": "image_url", "image_url": { "url": data_uri } }
                ]
            }
        ],
        "temperature": 0.1,
        "max_tokens": 512
    });

    let mut req = client.post(&endpoint).json(&body);
    if !api_key.trim().is_empty() {
        req = req.bearer_auth(api_key.trim());
    }

    let res: Result<Vec<VlmElement>, String> = rt.block_on(async {
        let resp = req.send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let raw_text = v.pointer("/choices/0/message/content").and_then(|v| v.as_str()).unwrap_or("");
        let clean = if let (Some(s), Some(e)) = (raw_text.find('['), raw_text.rfind(']')) {
            &raw_text[s..=e]
        } else {
            return Ok(Vec::new());
        };
        serde_json::from_str::<Vec<VlmElement>>(clean).map_err(|e| e.to_string())
    });

    let Ok(elements) = res else {
        return Vec::new();
    };

    convert_vlm_elements_to_targets(elements, window_rect, existing_targets)
}

fn convert_vlm_elements_to_targets(
    elements: Vec<VlmElement>,
    window_rect: [i32; 4],
    existing_targets: &[NumberedElementTarget],
) -> Vec<NumberedElementTarget> {

    let win_w = (window_rect[2] - window_rect[0]).max(1) as f32;
    let win_h = (window_rect[3] - window_rect[1]).max(1) as f32;

    let mut results = Vec::new();
    for elem in elements {
        let (rect, click_x, click_y) = if let Some([ymin, xmin, ymax, xmax]) = elem.box_2d {
            let x0 = window_rect[0] + (xmin * win_w / 1000.0).round() as i32;
            let y0 = window_rect[1] + (ymin * win_h / 1000.0).round() as i32;
            let x1 = window_rect[0] + (xmax * win_w / 1000.0).round() as i32;
            let y1 = window_rect[1] + (ymax * win_h / 1000.0).round() as i32;
            let cx = (x0 + x1) / 2;
            let cy = (y0 + y1) / 2;
            ([x0, y0, x1, y1], cx, cy)
        } else if let Some([px, py]) = elem.point {
            let cx = window_rect[0] + (px * win_w / 1000.0).round() as i32;
            let cy = window_rect[1] + (py * win_h / 1000.0).round() as i32;
            ([cx - 16, cy - 16, cx + 16, cy + 16], cx, cy)
        } else {
            continue;
        };

        let overlaps = existing_targets.iter().any(|t| {
            if t.name.trim().is_empty() { return false; }
            let inter_x0 = rect[0].max(t.rect[0]);
            let inter_y0 = rect[1].max(t.rect[1]);
            let inter_x1 = rect[2].min(t.rect[2]);
            let inter_y1 = rect[3].min(t.rect[3]);
            if inter_x1 > inter_x0 && inter_y1 > inter_y0 {
                let inter_area = ((inter_x1 - inter_x0) * (inter_y1 - inter_y0)) as f32;
                let area = ((rect[2] - rect[0]).max(1) * (rect[3] - rect[1]).max(1)) as f32;
                inter_area / area > 0.45
            } else {
                false
            }
        });

        if !overlaps {
            let role = if elem.role.trim().is_empty() {
                "bouton (Qwen2-VL)".to_string()
            } else {
                format!("{} (Qwen2-VL)", elem.role.trim())
            };
            let name = if elem.name.trim().is_empty() {
                "Élément visuel (VLM)".to_string()
            } else {
                elem.name.trim().to_string()
            };
            results.push(NumberedElementTarget {
                index: 0,
                click_x,
                click_y,
                rect,
                name,
                role,
                shortcut: "Entrée".to_string(),
                value: String::new(),
                window_id: None,
            });
        }
    }
    results
}

/// Palette de couleurs vives et contrastées pour différencier chaque zone visuelle détectée
pub fn visual_zone_color(index: usize) -> &'static str {
    const PALETTE: [&str; 12] = [
        "#FF3366", // Rose néon
        "#FF9900", // Orange vif
        "#FFD600", // Jaune ambré
        "#00E676", // Vert menthe
        "#00E5FF", // Cyan électrique
        "#2979FF", // Bleu azur
        "#7C4DFF", // Violet indigo
        "#E040FB", // Violet magenta
        "#FF1744", // Rouge corail
        "#76FF03", // Vert lime
        "#FF6D00", // Orange brûlé
        "#1DE9B6", // Sarcelle vif
    ];
    PALETTE[(index.saturating_sub(1)) % PALETTE.len()]
}

/// Enrichit les contrôles existants avec les textes et boutons détectés visuellement
pub fn enrich_targets_with_ocr(
    hwnd: HWND,
    window_rect: [i32; 4],
    existing_targets: &mut Vec<NumberedElementTarget>,
) {
    #[cfg(target_os = "macos")]
    {
        let vision_items = recognize_texts_vision(window_rect);
        if !vision_items.is_empty() {
            for (text, cand_rect) in vision_items {
                let overlaps = existing_targets.iter().any(|t| {
                    let is_container = t.role == "fenêtre"
                        || t.role == "window"
                        || t.role == "pane"
                        || t.role == "group"
                        || t.role == "groupe"
                        || t.role == "client"
                        || t.role == "panneau";
                    if is_container {
                        return false;
                    }
                    if t.name.trim().is_empty() {
                        return false;
                    }
                    let inter_x0 = cand_rect[0].max(t.rect[0]);
                    let inter_y0 = cand_rect[1].max(t.rect[1]);
                    let inter_x1 = cand_rect[2].min(t.rect[2]);
                    let inter_y1 = cand_rect[3].min(t.rect[3]);
                    if inter_x1 > inter_x0 && inter_y1 > inter_y0 {
                        let inter_area = ((inter_x1 - inter_x0) * (inter_y1 - inter_y0)) as f64;
                        let cand_area = ((cand_rect[2] - cand_rect[0]).max(1) * (cand_rect[3] - cand_rect[1]).max(1)) as f64;
                        inter_area / cand_area > 0.50
                    } else {
                        false
                    }
                });
                if !overlaps {
                    let click_x = (cand_rect[0] + cand_rect[2]) / 2;
                    let click_y = (cand_rect[1] + cand_rect[3]) / 2;
                    let next_index = existing_targets.len() + 1;
                    existing_targets.push(NumberedElementTarget {
                        index: next_index,
                        click_x,
                        click_y,
                        rect: cand_rect,
                        name: text,
                        role: "texte (Apple Vision)".to_string(),
                        shortcut: "Entrée".to_string(),
                        value: visual_zone_color(next_index).to_string(),
                        window_id: Some(hwnd.0 as isize),
                    });
                }
            }
            return;
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Some((pixels, w, h)) = capture_window_rgba(hwnd, window_rect) {
            let ocr_items = recognize_texts_ocrs(&pixels, w, h, window_rect);
            if !ocr_items.is_empty() {
                for (text, cand_rect) in ocr_items {
                    let overlaps = existing_targets.iter().any(|t| {
                        let is_container = t.role == "fenêtre"
                            || t.role == "window"
                            || t.role == "pane"
                            || t.role == "group"
                            || t.role == "groupe"
                            || t.role == "client"
                            || t.role == "panneau";
                        if is_container {
                            return false;
                        }
                        if t.name.trim().is_empty() {
                            return false;
                        }
                        let inter_x0 = cand_rect[0].max(t.rect[0]);
                        let inter_y0 = cand_rect[1].max(t.rect[1]);
                        let inter_x1 = cand_rect[2].min(t.rect[2]);
                        let inter_y1 = cand_rect[3].min(t.rect[3]);
                        if inter_x1 > inter_x0 && inter_y1 > inter_y0 {
                            let inter_area = ((inter_x1 - inter_x0) * (inter_y1 - inter_y0)) as f64;
                            let cand_area = ((cand_rect[2] - cand_rect[0]).max(1) * (cand_rect[3] - cand_rect[1]).max(1)) as f64;
                            inter_area / cand_area > 0.50
                        } else {
                            false
                        }
                    });
                    if !overlaps {
                        let click_x = (cand_rect[0] + cand_rect[2]) / 2;
                        let click_y = (cand_rect[1] + cand_rect[3]) / 2;
                        let next_index = existing_targets.len() + 1;
                        existing_targets.push(NumberedElementTarget {
                            index: next_index,
                            click_x,
                            click_y,
                            rect: cand_rect,
                            name: text,
                            role: "texte (ocrs)".to_string(),
                            shortcut: "Entrée".to_string(),
                            value: visual_zone_color(next_index).to_string(),
                            window_id: Some(hwnd.0 as isize),
                        });
                    }
                }
                return;
            }
        }
    }

    let Some((pixels, w, h)) = capture_window_rgba(hwnd, window_rect) else {
        return;
    };

    let candidate_boxes =
        detect_visual_boxes_from_pixels(&pixels, w, h, window_rect, existing_targets);

    // Injection dans la liste des cibles d'interaction
    for cand_rect in candidate_boxes {
        let click_x = (cand_rect[0] + cand_rect[2]) / 2;
        let click_y = (cand_rect[1] + cand_rect[3]) / 2;
        let next_index = existing_targets.len() + 1;

        let display_name = resolve_visual_zone_content(hwnd, cand_rect, click_x, click_y)
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("Zone visuelle {next_index}"));

        existing_targets.push(NumberedElementTarget {
            index: next_index,
            click_x,
            click_y,
            rect: cand_rect,
            name: display_name,
            role: "visuel (OCR)".to_string(),
            shortcut: "Entrée".to_string(),
            value: visual_zone_color(next_index).to_string(),
            window_id: Some(hwnd.0 as isize),
        });
    }
}