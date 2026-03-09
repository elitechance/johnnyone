use arboard::Clipboard;
use base64::{engine::general_purpose, Engine as _};
use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder};
use std::process::Command;

#[tauri::command]
pub fn read_clipboard_image_data_url() -> Result<Option<String>, String> {
    if let Some(data_url) = read_with_arboard()? {
        return Ok(Some(data_url));
    }

    if let Some(data_url) = read_with_wl_paste() {
        return Ok(Some(data_url));
    }

    if let Some(data_url) = read_with_xclip() {
        return Ok(Some(data_url));
    }

    if let Some(data_url) = read_with_windows_powershell() {
        return Ok(Some(data_url));
    }

    Ok(None)
}

fn read_with_arboard() -> Result<Option<String>, String> {
    let mut clipboard = match Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(_) => return Ok(None),
    };

    let image = match clipboard.get_image() {
        Ok(image) => image,
        Err(_) => return Ok(None),
    };

    let width =
        u32::try_from(image.width).map_err(|_| "clipboard image width overflow".to_string())?;
    let height =
        u32::try_from(image.height).map_err(|_| "clipboard image height overflow".to_string())?;
    let rgba = image.bytes.into_owned();
    encode_rgba_to_png_data_url(&rgba, width, height).map(Some)
}

fn read_with_wl_paste() -> Option<String> {
    let output = Command::new("wl-paste")
        .args(["--no-newline", "--type", "image/png"])
        .output()
        .ok()?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    let encoded = general_purpose::STANDARD.encode(output.stdout);
    Some(format!("data:image/png;base64,{encoded}"))
}

fn read_with_xclip() -> Option<String> {
    let output = Command::new("xclip")
        .args(["-selection", "clipboard", "-t", "image/png", "-o"])
        .output()
        .ok()?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    let encoded = general_purpose::STANDARD.encode(output.stdout);
    Some(format!("data:image/png;base64,{encoded}"))
}

fn read_with_windows_powershell() -> Option<String> {
    let script = r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
if ([System.Windows.Forms.Clipboard]::ContainsImage()) {
  $img = [System.Windows.Forms.Clipboard]::GetImage()
  $ms = New-Object System.IO.MemoryStream
  $img.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
  [Convert]::ToBase64String($ms.ToArray())
}
"#;

    let candidates = [
        "powershell.exe",
        "/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe",
    ];

    for candidate in candidates {
        let output = match Command::new(candidate)
            .args(["-NoProfile", "-NonInteractive", "-STA", "-Command", script])
            .output()
        {
            Ok(output) => output,
            Err(_) => continue,
        };

        if !output.status.success() || output.stdout.is_empty() {
            continue;
        }

        let encoded = decode_output_text(&output.stdout)?;
        if encoded.is_empty() {
            continue;
        }

        return Some(format!("data:image/png;base64,{encoded}"));
    }

    None
}

fn decode_output_text(bytes: &[u8]) -> Option<String> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if bytes.len() % 2 != 0 {
        return None;
    }

    let utf16: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    let text = String::from_utf16(&utf16).ok()?;
    let trimmed = text.trim_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(trimmed.to_string())
}

fn encode_rgba_to_png_data_url(rgba: &[u8], width: u32, height: u32) -> Result<String, String> {
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(rgba, width, height, ColorType::Rgba8.into())
        .map_err(|e| format!("png encode failed: {e}"))?;

    let encoded = general_purpose::STANDARD.encode(png);
    Ok(format!("data:image/png;base64,{encoded}"))
}
