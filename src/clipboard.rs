use crate::html_util::{decode_html_entities, extract_attribute, sanitize_html_to_markdown};
#[cfg(all(not(windows), not(target_os = "macos")))]
use crate::automation::is_wayland_session;

#[cfg(target_os = "macos")]
mod clipboard_platform {
    use std::io::Write;
    pub fn set_text(text: &str) -> bool {
        let mut child = match std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn() {
            Ok(c) => c,
            Err(_) => return false,
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        child.wait().map(|s| s.success()).unwrap_or(false)
    }
    pub fn get_text() -> Option<String> {
        let output = std::process::Command::new("pbpaste").output().ok()?;
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub fn get_plain_text() -> Option<String> {
        get_text()
    }

    pub fn get_advanced_text() -> Option<String> {
        if let Some(text) = get_text() {
            let lines: Vec<&str> = text.lines().collect();
            if !lines.is_empty() && lines.iter().all(|l| l.starts_with('/')) {
                let mut spans = Vec::new();
                for line in lines {
                    let path = line.trim();
                    if path.is_empty() { continue; }
                    let p = std::path::Path::new(path);
                    if p.exists() {
                        let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or(path);
                        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                        let is_img = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg");
                        let tag = if is_img {
                            format!("[link=\"action:Ouvrir l'image : {path}\"]🖼️ {fname}[/link]")
                        } else {
                            format!("[link=\"action:Ouvrir le fichier : {path}\"]📄 {fname}[/link]")
                        };
                        spans.push(tag);
                    }
                }
                if !spans.is_empty() {
                    return Some(spans.join(" "));
                }
            }
            return Some(text);
        }
        None
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
mod clipboard_platform {
    use std::io::Write;
    use super::html_to_advanced_markdown;

    fn copy_via_wl(text: &str, mime: &str) -> bool {
        let mut child = match std::process::Command::new("wl-copy")
            .args(["-t", mime])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return false,
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        child.wait().map(|s| s.success()).unwrap_or(false)
    }

    fn copy_via_xclip(text: &str, mime: &str) -> bool {
        let mut child = match std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-t", mime])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return false,
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        child.wait().map(|s| s.success()).unwrap_or(false)
    }

    pub fn set_text(text: &str) -> bool {
        let html_payload = super::convert_markdown_and_links_to_html(text);
        let has_links = text.contains("[link") || text.contains("](");
        let is_wayland = super::is_wayland_session();

        let ok = if is_wayland {
            copy_via_wl(text, "text/plain;charset=utf-8") || copy_via_xclip(text, "text/plain;charset=utf-8")
        } else {
            copy_via_xclip(text, "text/plain;charset=utf-8") || copy_via_wl(text, "text/plain;charset=utf-8")
        };

        if has_links {
            if is_wayland {
                if !copy_via_wl(&html_payload, "text/html") {
                    let _ = copy_via_xclip(&html_payload, "text/html");
                }
            } else if !copy_via_xclip(&html_payload, "text/html") {
                let _ = copy_via_wl(&html_payload, "text/html");
            }
        }
        ok
    }

    fn paste_format(mime: &str) -> Option<String> {
        let try_wl = || -> Option<String> {
            let output = std::process::Command::new("wl-paste")
                .args(["-t", mime])
                .output()
                .ok()?;
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).to_string();
                if !s.is_empty() { return Some(s); }
            }
            None
        };

        let try_xclip = || -> Option<String> {
            let output = std::process::Command::new("xclip")
                .args(["-selection", "clipboard", "-t", mime, "-o"])
                .output()
                .ok()?;
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).to_string();
                if !s.is_empty() { return Some(s); }
            }
            None
        };

        if super::is_wayland_session() {
            try_wl().or_else(try_xclip)
        } else {
            try_xclip().or_else(try_wl)
        }
    }

    pub fn get_advanced_text() -> Option<String> {
        if let Some(uris) = paste_format("text/uri-list") {
            let mut spans = Vec::new();
            for line in uris.lines() {
                let trimmed = line.trim();
                if let Some(path) = trimmed.strip_prefix("file://") {
                    let p = std::path::Path::new(path);
                    let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or(path);
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                    let is_img = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg");
                    let tag = if is_img {
                        format!("[link=\"action:Ouvrir l'image : {path}\"]🖼️ {fname}[/link]")
                    } else {
                        format!("[link=\"action:Ouvrir le fichier : {path}\"]📄 {fname}[/link]")
                    };
                    spans.push(tag);
                }
            }
            if !spans.is_empty() {
                return Some(spans.join(" "));
            }
        }

        if let Some(html) = paste_format("text/html") {
            let md = html_to_advanced_markdown(&html);
            if !md.trim().is_empty() {
                return Some(md);
            }
        }

        get_plain_text()
    }

    pub fn get_text() -> Option<String> {
        get_plain_text()
    }

    pub fn get_plain_text() -> Option<String> {
        let try_wl = || -> Option<String> {
            let output = std::process::Command::new("wl-paste")
                .args(["-t", "text/plain;charset=utf-8"])
                .output()
                .or_else(|_| std::process::Command::new("wl-paste").arg("--no-newline").output())
                .ok()?;
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).to_string();
                if !s.is_empty() { return Some(s); }
            }
            None
        };

        let try_xclip = || -> Option<String> {
            let output = std::process::Command::new("xclip")
                .args(["-selection", "clipboard", "-o"])
                .output()
                .ok()?;
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).to_string();
                if !s.is_empty() { return Some(s); }
            }
            None
        };

        if super::is_wayland_session() {
            try_wl().or_else(try_xclip)
        } else {
            try_xclip().or_else(try_wl)
        }
    }
}

#[cfg(windows)]
mod clipboard_platform {
    use std::ffi::c_void;

    extern "system" {
        fn OpenClipboard(hwnd: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(uformat: u32, hmem: *mut c_void) -> *mut c_void;
        fn GetClipboardData(uformat: u32) -> *mut c_void;
        fn IsClipboardFormatAvailable(uformat: u32) -> i32;
        fn GlobalAlloc(uflags: u32, dwbytes: usize) -> *mut c_void;
        fn GlobalSize(hmem: *mut c_void) -> usize;
        fn GlobalLock(hmem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(hmem: *mut c_void) -> i32;
        fn RegisterClipboardFormatW(lpszformat: *const u16) -> u32;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn DragQueryFileW(hdrop: *mut c_void, ifile: u32, lpfile: *mut u16, cch: u32) -> u32;
    }

    const CF_UNICODETEXT: u32 = 13;
    const CF_HDROP: u32 = 15;
    const GMEM_MOVEABLE: u32 = 0x0002;

    fn cf_html_format_id() -> u32 {
        let format_name: Vec<u16> = "HTML Format\0".encode_utf16().collect();
        unsafe { RegisterClipboardFormatW(format_name.as_ptr()) }
    }

    pub fn set_text(text: &str) -> bool {
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes_len = utf16.len() * std::mem::size_of::<u16>();
        unsafe {
            let mut opened = false;
            for _ in 0..10 {
                if OpenClipboard(std::ptr::null_mut()) != 0 {
                    opened = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if !opened {
                return false;
            }

            EmptyClipboard();
            let h_mem = GlobalAlloc(GMEM_MOVEABLE, bytes_len);
            if !h_mem.is_null() {
                let p_data = GlobalLock(h_mem) as *mut u16;
                if !p_data.is_null() {
                    std::ptr::copy_nonoverlapping(utf16.as_ptr(), p_data, utf16.len());
                    GlobalUnlock(h_mem);
                    SetClipboardData(CF_UNICODETEXT, h_mem);
                }
            }

            if text.contains("[link") || text.contains("](") {
                let cf_html = cf_html_format_id();
                if cf_html != 0 {
                    let html_body = super::convert_markdown_and_links_to_html(text);
                    let cf_html_payload = build_cf_html_buffer(&html_body);
                    let h_mem_html = GlobalAlloc(GMEM_MOVEABLE, cf_html_payload.len());
                    if !h_mem_html.is_null() {
                        let p_html = GlobalLock(h_mem_html) as *mut u8;
                        if !p_html.is_null() {
                            std::ptr::copy_nonoverlapping(cf_html_payload.as_ptr(), p_html, cf_html_payload.len());
                            GlobalUnlock(h_mem_html);
                            SetClipboardData(cf_html, h_mem_html);
                        }
                    }
                }
            }
            CloseClipboard();
        }
        true
    }

    fn build_cf_html_buffer(fragment: &str) -> Vec<u8> {
        let start_html = 105;
        let start_fragment = 139;
        let end_fragment = start_fragment + fragment.as_bytes().len();
        let footer = "<!--EndFragment-->\r\n</body></html>\0";
        let total_len = end_fragment + footer.len() - 1;

        let header = format!(
            "Version:0.9\r\nStartHTML:{:010}\r\nEndHTML:{:010}\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\n<html><body>\r\n<!--StartFragment-->",
            start_html,
            total_len,
            start_fragment,
            end_fragment
        );

        let mut buf = Vec::with_capacity(header.len() + fragment.len() + footer.len());
        buf.extend_from_slice(header.as_bytes());
        buf.extend_from_slice(fragment.as_bytes());
        buf.extend_from_slice(footer.as_bytes());
        buf
    }

    pub fn get_advanced_text() -> Option<String> {
        unsafe {
            let mut opened = false;
            for _ in 0..10 {
                if OpenClipboard(std::ptr::null_mut()) != 0 {
                    opened = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if !opened {
                return None;
            }

            if IsClipboardFormatAvailable(CF_HDROP) != 0 {
                let h_drop = GetClipboardData(CF_HDROP);
                if !h_drop.is_null() {
                    let count = DragQueryFileW(h_drop, 0xFFFFFFFF, std::ptr::null_mut(), 0);
                    if count > 0 {
                        let mut file_spans = Vec::new();
                        for i in 0..count {
                            let len = DragQueryFileW(h_drop, i, std::ptr::null_mut(), 0);
                            if len > 0 {
                                let mut buf = vec![0u16; (len + 1) as usize];
                                let copied = DragQueryFileW(h_drop, i, buf.as_mut_ptr(), len + 1);
                                if copied > 0 {
                                    let path = String::from_utf16_lossy(&buf[..copied as usize]);
                                    let p = std::path::Path::new(&path);
                                    let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or(&path);
                                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

                                    let is_img = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg");
                                    let is_audio = matches!(ext.as_str(), "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac");
                                    let is_video = matches!(ext.as_str(), "mp4" | "mkv" | "avi" | "mov" | "webm" | "wmv");

                                    let tag = if is_img {
                                        format!("[link=\"action:Ouvrir l'image : {path}\"]🖼️ {fname}[/link]")
                                    } else if is_audio {
                                        format!("[link=\"action:Lancer l'audio : {path}\"]🎵 {fname}[/link]")
                                    } else if is_video {
                                        format!("[link=\"action:Lancer la vidéo : {path}\"]▶ {fname}[/link]")
                                    } else {
                                        format!("[link=\"action:Ouvrir le fichier : {path}\"]📄 {fname}[/link]")
                                    };
                                    file_spans.push(tag);
                                }
                            }
                        }
                        let _ = CloseClipboard();
                        if !file_spans.is_empty() {
                            return Some(file_spans.join(" "));
                        }
                        return None;
                    }
                }
            }

            let cf_html = cf_html_format_id();
            if cf_html != 0 && IsClipboardFormatAvailable(cf_html) != 0 {
                let h_mem = GetClipboardData(cf_html);
                if !h_mem.is_null() {
                    let p_data = GlobalLock(h_mem) as *const u8;
                    if !p_data.is_null() {
                        let size = GlobalSize(h_mem);
                        let mut len = 0;
                        while len < size && *p_data.add(len) != 0 {
                            len += 1;
                        }
                        let slice = std::slice::from_raw_parts(p_data, len);
                        let raw_html = String::from_utf8_lossy(slice);
                        GlobalUnlock(h_mem);

                        let md = super::parse_cf_html_to_markdown(&raw_html);
                        if !md.trim().is_empty() {
                            let _ = CloseClipboard();
                            return Some(md);
                        }
                    }
                }
            }

            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
                let _ = CloseClipboard();
                return None;
            }

            let h_mem = GetClipboardData(CF_UNICODETEXT);
            if h_mem.is_null() {
                let _ = CloseClipboard();
                return None;
            }

            let p_data = GlobalLock(h_mem) as *const u16;
            if p_data.is_null() {
                let _ = CloseClipboard();
                return None;
            }

            let mut len = 0;
            while *p_data.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(p_data, len);
            let s = String::from_utf16_lossy(slice);
            let _ = GlobalUnlock(h_mem);
            let _ = CloseClipboard();
            Some(s)
        }
    }

    pub fn get_plain_text() -> Option<String> {
        unsafe {
            let mut opened = false;
            for _ in 0..10 {
                if OpenClipboard(std::ptr::null_mut()) != 0 {
                    opened = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if !opened {
                return None;
            }

            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
                let _ = CloseClipboard();
                return None;
            }

            let h_mem = GetClipboardData(CF_UNICODETEXT);
            if h_mem.is_null() {
                let _ = CloseClipboard();
                return None;
            }

            let p_data = GlobalLock(h_mem) as *const u16;
            if p_data.is_null() {
                let _ = CloseClipboard();
                return None;
            }

            let mut len = 0;
            while *p_data.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(p_data, len);
            let s = String::from_utf16_lossy(slice);
            let _ = GlobalUnlock(h_mem);
            let _ = CloseClipboard();
            Some(s)
        }
    }

    pub fn get_text() -> Option<String> {
        get_plain_text()
    }
}

pub fn get_clipboard_text() -> Option<String> {
    clipboard_platform::get_advanced_text().or_else(clipboard_platform::get_plain_text)
}

pub fn get_plain_clipboard_text() -> Option<String> {
    clipboard_platform::get_plain_text()
}

pub fn set_clipboard_text(text: &str) -> bool {
    clipboard_platform::set_text(text)
}

pub use clipboard_platform::{get_advanced_text, get_plain_text, get_text, set_text};

pub fn html_to_advanced_markdown(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut i = 0;
    let bytes = html.as_bytes();
    let len = bytes.len();

    let mut current_link_href: Option<String> = None;
    let mut current_link_text = String::new();
    let mut has_links = false;

    while i < len {
        if bytes[i] == b'<' {
            if let Some(tag_end) = html[i..].find('>') {
                let tag_content = html[i + 1..i + tag_end].trim();
                let is_closing = tag_content.starts_with('/');
                let tag_body = if is_closing { tag_content[1..].trim() } else { tag_content };
                let tag_name = tag_body.split_whitespace().next().unwrap_or("").to_lowercase();
                let tag_name = tag_name.trim_end_matches('/');

                if tag_name == "a" {
                    if is_closing {
                        if let Some(href) = current_link_href.take() {
                            let label = current_link_text.trim();
                            if !label.is_empty() {
                                result.push_str(&format!("[link=\"{href}\"]{label}[/link]"));
                                has_links = true;
                            } else if !href.is_empty() {
                                result.push_str(&format!("[link=\"{href}\"]{href}[/link]"));
                                has_links = true;
                            }
                            current_link_text.clear();
                        }
                    } else if let Some(href) = extract_attribute(tag_body, "href") {
                        let clean_href = decode_html_entities(href.trim());
                        current_link_href = Some(clean_href);
                        current_link_text.clear();
                    }
                } else if tag_name == "br" || tag_name == "p" || tag_name == "div" || tag_name == "tr" || tag_name == "li" {
                    result.push(' ');
                } else if tag_name == "b" || tag_name == "strong" {
                    result.push_str("**");
                } else if tag_name == "code" {
                    result.push('`');
                }

                i += tag_end + 1;
                continue;
            }
        }

        if let Some(ch) = html[i..].chars().next() {
            if current_link_href.is_some() {
                current_link_text.push(ch);
            } else {
                result.push(ch);
            }
            i += ch.len_utf8();
        } else {
            break;
        }
    }

    if let Some(href) = current_link_href.take() {
        let label = current_link_text.trim();
        if !label.is_empty() {
            result.push_str(&format!("[link=\"{href}\"]{label}[/link]"));
            has_links = true;
        }
    }

    if has_links {
        let decoded = decode_html_entities(&result);
        decoded.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        String::new()
    }
}

pub fn parse_cf_html_to_markdown(raw_html: &str) -> String {
    let fragment = if let (Some(s), Some(e)) = (raw_html.find("<!--StartFragment-->"), raw_html.find("<!--EndFragment-->")) {
        if s < e {
            &raw_html[s + "<!--StartFragment-->".len()..e]
        } else {
            raw_html
        }
    } else {
        raw_html
    };

    html_to_advanced_markdown(fragment)
}

pub fn convert_markdown_and_links_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    out.push_str("<div style=\"font-family:sans-serif;\">\r\n");
    for line in text.lines() {
        let clean = crate::floating_window::render_markdown_line_to_html(line);
        out.push_str(&clean);
        out.push_str("<br/>\r\n");
    }
    out.push_str("</div>");
    out
}