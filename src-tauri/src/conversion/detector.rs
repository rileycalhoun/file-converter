use std::{fs::File, io::Read, path::Path};

use super::registry::{DetectedFormat, DetectionConfidence, InputFamily, InputFormat};
use super::ConversionError;

const HEADER_LIMIT: u64 = 16 * 1024;
const OLE_SIGNATURE: &[u8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1";

pub fn detect_format(path: &Path) -> Result<DetectedFormat, ConversionError> {
    if !path.is_file() {
        return Err(ConversionError::SourceNotFound(path.to_path_buf()));
    }
    let mut header = Vec::new();
    File::open(path)
        .map_err(|source| ConversionError::ReadSource {
            path: path.to_path_buf(),
            source,
        })?
        .take(HEADER_LIMIT)
        .read_to_end(&mut header)
        .map_err(|source| ConversionError::ReadSource {
            path: path.to_path_buf(),
            source,
        })?;
    detect_format_from_bytes(path, &header)
}

pub(crate) fn detect_format_from_bytes(
    path: &Path,
    header: &[u8],
) -> Result<DetectedFormat, ConversionError> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .and_then(InputFormat::from_extension);

    if let Some(format) = magic_format(header) {
        if extension.is_some_and(|candidate| !compatible_magic(candidate, format)) {
            return Err(ConversionError::DetectionFailed(format!(
                "The file contents look like {}, but the filename uses .{}.",
                format.extension(),
                path.extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
            )));
        }
        return Ok(DetectedFormat {
            format,
            confidence: DetectionConfidence::Magic,
        });
    }

    let Some(format) = extension else {
        return Err(ConversionError::UnsupportedFormat(
            path.extension()
                .and_then(|value| value.to_str())
                .unwrap_or("unknown")
                .to_string(),
        ));
    };

    let confidence = match format {
        InputFormat::Doc | InputFormat::Xls | InputFormat::Ppt => {
            if !header.starts_with(OLE_SIGNATURE) {
                return Err(ConversionError::DetectionFailed(
                    "This legacy Office file does not have a valid compound-document signature."
                        .into(),
                ));
            }
            DetectionConfidence::Container
        }
        InputFormat::Docx
        | InputFormat::Xlsx
        | InputFormat::Pptx
        | InputFormat::Odt
        | InputFormat::Ods
        | InputFormat::Odp
        | InputFormat::Odg
        | InputFormat::Odf
        | InputFormat::Epub => {
            if !header.starts_with(b"PK") {
                return Err(ConversionError::DetectionFailed(
                    "This file does not have the ZIP container expected for its format.".into(),
                ));
            }
            DetectionConfidence::Container
        }
        InputFormat::Rtf => {
            if !trim_ascii_start(header).starts_with(b"{\\rtf") {
                return Err(ConversionError::DetectionFailed(
                    "This file does not have a valid RTF header.".into(),
                ));
            }
            DetectionConfidence::Magic
        }
        InputFormat::Html => {
            if !looks_like_html(header) {
                return Err(ConversionError::DetectionFailed(
                    "This file does not look like an HTML document.".into(),
                ));
            }
            DetectionConfidence::Magic
        }
        InputFormat::Txt | InputFormat::Csv => DetectionConfidence::Extension,
        _ => {
            return Err(ConversionError::DetectionFailed(
                "The file signature does not match its extension.".into(),
            ));
        }
    };

    Ok(DetectedFormat { format, confidence })
}

fn magic_format(bytes: &[u8]) -> Option<InputFormat> {
    if bytes.starts_with(b"%PDF-") {
        Some(InputFormat::Pdf)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1A\n") {
        Some(InputFormat::Png)
    } else if bytes.starts_with(b"\xFF\xD8\xFF") {
        Some(InputFormat::Jpeg)
    } else if bytes.starts_with(b"BM") {
        Some(InputFormat::Bmp)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(InputFormat::Gif)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some(InputFormat::Tiff)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(InputFormat::Webp)
    } else {
        None
    }
}

fn compatible_magic(extension: InputFormat, magic: InputFormat) -> bool {
    extension == magic
        || (extension.family() == InputFamily::Image
            && extension == InputFormat::Jpeg
            && magic == InputFormat::Jpeg)
}

fn trim_ascii_start(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    bytes
}

fn looks_like_html(bytes: &[u8]) -> bool {
    let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    let prefix = prefix.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    prefix.starts_with("<!doctype html") || prefix.starts_with("<html")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::detect_format_from_bytes;
    use crate::conversion::registry::InputFormat;

    #[test]
    fn detects_magic_before_extension() {
        let png = b"\x89PNG\r\n\x1A\nrest";
        assert_eq!(
            detect_format_from_bytes(Path::new("image.png"), png)
                .unwrap()
                .format,
            InputFormat::Png
        );
        assert!(detect_format_from_bytes(Path::new("image.docx"), png).is_err());
    }

    #[test]
    fn requires_container_signature_for_modern_office_files() {
        assert_eq!(
            detect_format_from_bytes(Path::new("report.docx"), b"PK\x03\x04")
                .unwrap()
                .format,
            InputFormat::Docx
        );
        assert!(detect_format_from_bytes(Path::new("report.docx"), b"not a zip").is_err());
    }

    #[test]
    fn detects_textual_formats_conservatively() {
        assert_eq!(
            detect_format_from_bytes(Path::new("notes.rtf"), b"{\\rtf1 hello}")
                .unwrap()
                .format,
            InputFormat::Rtf
        );
        assert_eq!(
            detect_format_from_bytes(Path::new("page.html"), b"<!doctype html><p>Hi</p>")
                .unwrap()
                .format,
            InputFormat::Html
        );
    }

    #[test]
    fn rejects_unknown_extensions() {
        assert!(detect_format_from_bytes(Path::new("archive.xyz"), b"data").is_err());
    }
}
