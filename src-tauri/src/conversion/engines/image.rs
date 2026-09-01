use async_trait::async_trait;
use printpdf::{Mm, Op, PdfDocument, PdfPage, PdfSaveOptions, Pt, RawImage, XObjectTransform};

use crate::conversion::{
    ConversionEngine, ConversionError, ConversionRequest, ConversionResult, DetectedFormat,
    EngineOutput, InputFamily,
};

pub struct ImagePdfEngine;

#[async_trait]
impl ConversionEngine for ImagePdfEngine {
    fn id(&self) -> &'static str {
        "image-pdf"
    }

    fn supports(&self, input: &DetectedFormat) -> bool {
        input.format.family() == InputFamily::Image
    }

    async fn convert(&self, request: &ConversionRequest) -> Result<EngineOutput, ConversionError> {
        let source_path = request.source_path.clone();
        let output_path = request.staged_output_path();
        let source_name = request.source_name.clone();
        let output_size = tokio::task::spawn_blocking(move || {
            let bytes =
                std::fs::read(&source_path).map_err(|source| ConversionError::ReadSource {
                    path: source_path.clone(),
                    source,
                })?;
            let mut warnings = Vec::new();
            let image = RawImage::decode_from_bytes(&bytes, &mut warnings).map_err(|error| {
                ConversionError::ConversionFailed(format!(
                    "The image could not be decoded: {error}"
                ))
            })?;

            let (page_width, page_height) = if image.width > image.height {
                (Mm(297.0), Mm(210.0))
            } else {
                (Mm(210.0), Mm(297.0))
            };
            let margin_pt = Mm(10.0).into_pt().0;
            let available_width = page_width.into_pt().0 - margin_pt * 2.0;
            let available_height = page_height.into_pt().0 - margin_pt * 2.0;
            let native_width = image.width as f32 * 72.0 / 300.0;
            let native_height = image.height as f32 * 72.0 / 300.0;
            let scale = (available_width / native_width)
                .min(available_height / native_height)
                .min(1.0);
            let rendered_width = native_width * scale;
            let rendered_height = native_height * scale;

            let mut document = PdfDocument::new(&source_name);
            let image_id = document.add_image(&image);
            let page = PdfPage::new(
                page_width,
                page_height,
                vec![Op::UseXobject {
                    id: image_id,
                    transform: XObjectTransform {
                        translate_x: Some(Pt((page_width.into_pt().0 - rendered_width) / 2.0)),
                        translate_y: Some(Pt((page_height.into_pt().0 - rendered_height) / 2.0)),
                        scale_x: Some(scale),
                        scale_y: Some(scale),
                        dpi: Some(300.0),
                        ..Default::default()
                    },
                }],
            );
            let pdf = document
                .with_pages(vec![page])
                .save(&PdfSaveOptions::default(), &mut warnings);
            std::fs::write(&output_path, &pdf).map_err(|source| {
                ConversionError::OutputWriteFailed {
                    path: output_path,
                    source,
                }
            })?;
            Ok::<u64, ConversionError>(pdf.len() as u64)
        })
        .await
        .map_err(|error| ConversionError::ConversionFailed(error.to_string()))??;

        Ok(EngineOutput::Complete(ConversionResult {
            engine: self.id().into(),
            output_size,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::PathBuf;

    use super::ImagePdfEngine;
    use crate::conversion::{detect_format, ConversionEngine, ConversionRequest, EngineOutput};

    #[tokio::test]
    async fn converts_a_generated_png_to_pdf() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("pixel.png");
        // Reuse the repository-owned app icon as a small licensed PNG fixture.
        let png = include_bytes!("../../../../static/favicon.png");
        std::fs::write(&source, png).unwrap();
        let detected = detect_format(&source).unwrap();
        let request = ConversionRequest {
            id: uuid::Uuid::new_v4(),
            source_name: "pixel.png".into(),
            source_path: source,
            detected,
            output_path: directory.path().join("pixel.pdf"),
            work_directory: PathBuf::from(directory.path()),
            source_size: png.len() as u64,
        };
        let result = ImagePdfEngine.convert(&request).await.unwrap();
        assert!(matches!(result, EngineOutput::Complete(_)));
        assert!(std::fs::read(request.staged_output_path())
            .unwrap()
            .starts_with(b"%PDF-"));
    }

    #[tokio::test]
    async fn converts_a_generated_jpeg_to_pdf() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("pixel.jpg");
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            2,
            2,
            image::Rgb([32, 96, 160]),
        ))
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
        std::fs::write(&source, jpeg.get_ref()).unwrap();
        let detected = detect_format(&source).unwrap();
        let request = ConversionRequest {
            id: uuid::Uuid::new_v4(),
            source_name: "pixel.jpg".into(),
            source_path: source,
            detected,
            output_path: directory.path().join("pixel.pdf"),
            work_directory: PathBuf::from(directory.path()),
            source_size: jpeg.get_ref().len() as u64,
        };
        ImagePdfEngine.convert(&request).await.unwrap();
        assert!(std::fs::read(request.staged_output_path())
            .unwrap()
            .starts_with(b"%PDF-"));
    }
}
