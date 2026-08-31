use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputFamily {
    Document,
    Spreadsheet,
    Presentation,
    Drawing,
    Text,
    Image,
    Pdf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputFormat {
    Doc,
    Docx,
    Xls,
    Xlsx,
    Ppt,
    Pptx,
    Odt,
    Ods,
    Odp,
    Odg,
    Odf,
    Rtf,
    Txt,
    Html,
    Csv,
    Epub,
    Pdf,
    Png,
    Jpeg,
    Bmp,
    Gif,
    Tiff,
    Webp,
}

impl InputFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Doc => "doc",
            Self::Docx => "docx",
            Self::Xls => "xls",
            Self::Xlsx => "xlsx",
            Self::Ppt => "ppt",
            Self::Pptx => "pptx",
            Self::Odt => "odt",
            Self::Ods => "ods",
            Self::Odp => "odp",
            Self::Odg => "odg",
            Self::Odf => "odf",
            Self::Rtf => "rtf",
            Self::Txt => "txt",
            Self::Html => "html",
            Self::Csv => "csv",
            Self::Epub => "epub",
            Self::Pdf => "pdf",
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Bmp => "bmp",
            Self::Gif => "gif",
            Self::Tiff => "tiff",
            Self::Webp => "webp",
        }
    }

    pub const fn family(self) -> InputFamily {
        match self {
            Self::Doc | Self::Docx | Self::Odt | Self::Epub => InputFamily::Document,
            Self::Xls | Self::Xlsx | Self::Ods => InputFamily::Spreadsheet,
            Self::Ppt | Self::Pptx | Self::Odp => InputFamily::Presentation,
            Self::Odg | Self::Odf => InputFamily::Drawing,
            Self::Rtf | Self::Txt | Self::Html | Self::Csv => InputFamily::Text,
            Self::Pdf => InputFamily::Pdf,
            Self::Png | Self::Jpeg | Self::Bmp | Self::Gif | Self::Tiff | Self::Webp => {
                InputFamily::Image
            }
        }
    }

    pub fn from_extension(extension: &str) -> Option<Self> {
        Some(
            match extension
                .trim_start_matches('.')
                .to_ascii_lowercase()
                .as_str()
            {
                "doc" => Self::Doc,
                "docx" => Self::Docx,
                "xls" => Self::Xls,
                "xlsx" => Self::Xlsx,
                "ppt" => Self::Ppt,
                "pptx" => Self::Pptx,
                "odt" => Self::Odt,
                "ods" => Self::Ods,
                "odp" => Self::Odp,
                "odg" => Self::Odg,
                "odf" => Self::Odf,
                "rtf" => Self::Rtf,
                "txt" => Self::Txt,
                "html" | "htm" => Self::Html,
                "csv" => Self::Csv,
                "epub" => Self::Epub,
                "pdf" => Self::Pdf,
                "png" => Self::Png,
                "jpg" | "jpeg" => Self::Jpeg,
                "bmp" => Self::Bmp,
                "gif" => Self::Gif,
                "tif" | "tiff" => Self::Tiff,
                "webp" => Self::Webp,
                _ => return None,
            },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectedFormat {
    pub format: InputFormat,
    pub confidence: DetectionConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionConfidence {
    Magic,
    Container,
    Extension,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportedFormat {
    pub format: &'static str,
    pub label: &'static str,
    pub extensions: &'static [&'static str],
    pub family: InputFamily,
    pub engine: &'static str,
}

// This registry intentionally mirrors @matbee/libreoffice-converter 2.7.2's declared
// browser InputFormat union, plus formats implemented and tested by the native engines.
pub const SUPPORTED_FORMATS: &[SupportedFormat] = &[
    office(InputFormat::Doc, "Microsoft Word 97–2003", &["doc"]),
    office(InputFormat::Docx, "Microsoft Word", &["docx"]),
    office(InputFormat::Xls, "Microsoft Excel 97–2003", &["xls"]),
    office(InputFormat::Xlsx, "Microsoft Excel", &["xlsx"]),
    office(InputFormat::Ppt, "Microsoft PowerPoint 97–2003", &["ppt"]),
    office(InputFormat::Pptx, "Microsoft PowerPoint", &["pptx"]),
    office(InputFormat::Odt, "OpenDocument Text", &["odt"]),
    office(InputFormat::Ods, "OpenDocument Spreadsheet", &["ods"]),
    office(InputFormat::Odp, "OpenDocument Presentation", &["odp"]),
    office(InputFormat::Odg, "OpenDocument Drawing", &["odg"]),
    office(InputFormat::Odf, "OpenDocument Formula", &["odf"]),
    office(InputFormat::Rtf, "Rich Text Format", &["rtf"]),
    office(InputFormat::Txt, "Plain Text", &["txt"]),
    office(InputFormat::Html, "HTML", &["html", "htm"]),
    office(InputFormat::Csv, "Comma-Separated Values", &["csv"]),
    office(InputFormat::Epub, "EPUB", &["epub"]),
    native(InputFormat::Png, "PNG image", &["png"]),
    native(InputFormat::Jpeg, "JPEG image", &["jpg", "jpeg"]),
    native(InputFormat::Bmp, "BMP image", &["bmp"]),
    native(InputFormat::Gif, "GIF image", &["gif"]),
    native(InputFormat::Tiff, "TIFF image", &["tif", "tiff"]),
    native(InputFormat::Webp, "WebP image", &["webp"]),
    SupportedFormat {
        format: "pdf",
        label: "PDF (copy without re-encoding)",
        extensions: &["pdf"],
        family: InputFamily::Pdf,
        engine: "pdf-passthrough",
    },
];

const fn office(
    format: InputFormat,
    label: &'static str,
    extensions: &'static [&'static str],
) -> SupportedFormat {
    SupportedFormat {
        format: format.extension(),
        label,
        extensions,
        family: format.family(),
        engine: "libreoffice-wasm",
    }
}

const fn native(
    format: InputFormat,
    label: &'static str,
    extensions: &'static [&'static str],
) -> SupportedFormat {
    SupportedFormat {
        format: format.extension(),
        label,
        extensions,
        family: format.family(),
        engine: "image-pdf",
    }
}

pub fn supported_formats() -> &'static [SupportedFormat] {
    SUPPORTED_FORMATS
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{supported_formats, InputFormat};

    #[test]
    fn registry_has_unique_extensions_and_known_formats() {
        let mut extensions = HashSet::new();
        for entry in supported_formats() {
            for extension in entry.extensions {
                assert!(
                    extensions.insert(*extension),
                    "duplicate extension: {extension}"
                );
                assert!(InputFormat::from_extension(extension).is_some());
            }
        }
        assert!(extensions.contains("docx"));
        assert!(extensions.contains("png"));
        assert!(extensions.contains("pdf"));
        assert!(!extensions.contains("docm"));
    }
}
