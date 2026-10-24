use crate::automation::NumberedElementTarget;
#[cfg(windows)]
use windows::Win32::Foundation::{HWND, POINT};
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, SRCCOPY,
};
#[cfg(windows)]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
#[cfg(windows)]
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextW, WindowFromPoint};

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

    unsafe {
        let hdc_screen = GetDC(HWND(std::ptr::null_mut()));
        if hdc_screen.0.is_null() {
            return None;
        }
        let hdc_mem = CreateCompatibleDC(hdc_screen);
        let hbm = CreateCompatibleBitmap(hdc_screen, w as i32, h as i32);
        let old_bm = SelectObject(hdc_mem, hbm);

        let _ = BitBlt(
            hdc_mem, 0, 0, w as i32, h as i32,
            hdc_screen, rect[0], rect[1], SRCCOPY,
        );

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
        extern "system" {
            fn GetDIBits(
                hdc: *mut std::ffi::c_void,
                hbm: *mut std::ffi::c_void,
                start: u32,
                c_lines: u32,
                lpv_bits: *mut u8,
                lpbmi: *mut BITMAPINFO,
                usage: u32,
            ) -> i32;
        }

        let lines = GetDIBits(
            hdc_mem.0, hbm.0, 0, h, buffer.as_mut_ptr(), &mut bmi, DIB_RGB_COLORS.0,
        );

        SelectObject(hdc_mem, old_bm);
        DeleteObject(hbm);
        DeleteDC(hdc_mem);
        ReleaseDC(HWND(std::ptr::null_mut()), hdc_screen);

        if lines > 0 {
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

            // Niveau de reconnaissance rapide pour réactivité d'interface (1 = fast, 0 = accurate)
            let sel_set_level = sel_registerName(b"setRecognitionLevel:\0".as_ptr() as *const _);
            let set_level_fn: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, isize) =
                std::mem::transmute(objc_msgSend as *const ());
            set_level_fn(req, sel_set_level, 1);

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

                    let count = count_fn(obs_array, sel_count).min(40);
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
    use std::sync::OnceLock;

    static ENGINE: OnceLock<Option<ocrs::OcrEngine>> = OnceLock::new();

    ENGINE.get_or_init(|| {
        fn find_model(name: &str) -> Option<PathBuf> {
            let mut candidates = vec![
            PathBuf::from(name),
            PathBuf::from("models").join(name),
            PathBuf::from("/usr/share/libertide/models").join(name),
        ];
        if let Ok(home) = std::env::var("HOME") {
            candidates.push(PathBuf::from(home).join(".cache/libertide/models").join(name));
        }
        candidates.into_iter().find(|p| p.is_file())
    }

        let det_path = find_model("text-detection.rten")?;
        let rec_path = find_model("text-recognition.rten")?;

        let det_model = rten::Model::load_file(&det_path).ok()?;
        let rec_model = rten::Model::load_file(&rec_path).ok()?;

        ocrs::OcrEngine::new(ocrs::OcrEngineParams {
            detection_model: Some(det_model),
            recognition_model: Some(rec_model),
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
                    let r = word.bounding_rect();
                    min_x = min_x.min(r.left() as i32);
                    min_y = min_y.min(r.top() as i32);
                    max_x = max_x.max(r.right() as i32);
                    max_y = max_y.max(r.bottom() as i32);
                }
                if min_x < max_x && min_y < max_y {
                    let x0 = window_rect[0] + min_x;
                    let y0 = window_rect[1] + min_y;
                    let x1 = window_rect[0] + max_x;
                    let y1 = window_rect[1] + max_y;
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

/// Capture native sous Linux via X11 / XGetImage (< 4 ms)
#[cfg(target_os = "linux")]
pub fn capture_window_rgba(_hwnd: HWND, rect: [i32; 4]) -> Option<(Vec<u8>, u32, u32)> {
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
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
            let sample_points = [
                POINT { x: click_x, y: click_y },
                POINT { x: (rect[0] + 10).min(rect[2]), y: click_y },
                POINT { x: (rect[2] - 10).max(rect[0]), y: click_y },
                POINT { x: click_x, y: (rect[1] + 6).min(rect[3]) },
            ];

            for pt in sample_points {
                if let Ok(elem) = uia.ElementFromPoint(pt) {
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
        if !child.is_invalid() && child != hwnd {
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
    if w < 30 || h < 30 {
        return Vec::new();
    }

    let required_len = (w as usize) * (h as usize) * 4;
    if pixels.len() < required_len {
        return Vec::new();
    }

    // Découpage en cellules pour détection rapide des contrastes de contours (texte / boutons)
    const CELL_W: u32 = 16;
    const CELL_H: u32 = 12;

    let grid_cols = (w / CELL_W) as usize;
    let grid_rows = (h / CELL_H) as usize;
    if grid_cols == 0 || grid_rows == 0 {
        return Vec::new();
    }

    let mut grid = vec![false; grid_cols * grid_rows];

    // Échantillonnage des transitions horizontales de luminance dans chaque cellule
    for gy in 0..grid_rows {
        for gx in 0..grid_cols {
            let x_start = (gx as u32) * CELL_W;
            let y_start = (gy as u32) * CELL_H;
            let mut edge_count = 0;
            let mut samples = 0;

            for dy in (0..CELL_H).step_by(2) {
                let py = y_start + dy;
                if py >= h { break; }
                let row_offset = (py * w * 4) as usize;

                for dx in (0..CELL_W.saturating_sub(1)).step_by(2) {
                    let px = x_start + dx;
                    if px + 1 >= w { break; }
                    let idx1 = row_offset + (px as usize) * 4;
                    let idx2 = row_offset + ((px + 1) as usize) * 4;

                    let b1 = pixels[idx1] as i32;
                    let g1 = pixels[idx1 + 1] as i32;
                    let r1 = pixels[idx1 + 2] as i32;
                    let lum1 = (r1 * 299 + g1 * 587 + b1 * 114) >> 10;

                    let b2 = pixels[idx2] as i32;
                    let g2 = pixels[idx2 + 1] as i32;
                    let r2 = pixels[idx2 + 2] as i32;
                    let lum2 = (r2 * 299 + g2 * 587 + b2 * 114) >> 10;

                    if (lum1 - lum2).abs() > 26 {
                        edge_count += 1;
                    }
                    samples += 1;
                }
            }

            // Une zone de texte/bouton présente une densité de contours entre 8% et 65%
            if samples > 0 && edge_count >= 2 && edge_count <= (samples * 65 / 100) {
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

                // Exploration des voisins immédiats et pontage horizontal (+/- 2 cellules)
                for dy in [-1isize, 0, 1] {
                    let ny = cy as isize + dy;
                    if ny < 0 || ny >= grid_rows as isize { continue; }
                    for dx in [-2isize, -1, 0, 1, 2] {
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

            // Filtrage : taille compatible avec du texte ou des boutons d'interface
            if box_w >= 28 && box_h >= 10 && box_h <= 90 && box_w <= (w * 4 / 5) {
                let screen_x0 = window_rect[0] + (min_gx as u32 * CELL_W) as i32;
                let screen_y0 = window_rect[1] + (min_gy as u32 * CELL_H) as i32;
                let screen_x1 = screen_x0 + box_w as i32;
                let screen_y1 = screen_y0 + box_h as i32;

                // Vérifier qu'elle n'intersecte aucun contrôle UIA déjà recensé
                let overlaps_uia = existing_targets.iter().any(|t| {
                    screen_x0 < t.rect[2]
                        && screen_x1 > t.rect[0]
                        && screen_y0 < t.rect[3]
                        && screen_y1 > t.rect[1]
                });

                if !overlaps_uia {
                    candidate_boxes.push([screen_x0, screen_y0, screen_x1, screen_y1]);
                }
            }
        }
    }

    // Trier les cibles par ordre visuel de lecture (haut-vers-bas, gauche-vers-droite)
    candidate_boxes.sort_by_key(|b| (b[1] / 20) * 10000 + b[0]);
    candidate_boxes.truncate(25);

    candidate_boxes
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
                    cand_rect[0] < t.rect[2]
                        && cand_rect[2] > t.rect[0]
                        && cand_rect[1] < t.rect[3]
                        && cand_rect[3] > t.rect[1]
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
                        value: String::new(),
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
                        cand_rect[0] < t.rect[2]
                            && cand_rect[2] > t.rect[0]
                            && cand_rect[1] < t.rect[3]
                            && cand_rect[3] > t.rect[1]
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
                            value: String::new(),
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
            value: String::new(),
            window_id: Some(hwnd.0 as isize),
        });
    }
}