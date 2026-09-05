use async_trait::async_trait;
use image::{DynamicImage, ImageDecoder, ImageReader};
use printpdf::{
    ImageCompression, ImageOptimizationOptions, Mm, Op, PdfDocument, PdfPage, PdfSaveOptions, Pt,
    RawImage, RawImageData, RawImageFormat, XObjectTransform,
};

use crate::conversion::{
    ConversionEngine, ConversionError, ConversionRequest, ConversionResult, DetectedFormat,
    EngineOutput, InputFamily,
};

pub struct ImagePdfEngine;

// Physical placement only: fitting to A4 changes the PDF transform, never the pixels.
const IMAGE_LAYOUT_DPI: f32 = 300.0;

fn preserve_detail_save_options() -> PdfSaveOptions {
    PdfSaveOptions {
        optimize: true,
        subset_fonts: true,
        secure: true,
        image_optimization: Some(ImageOptimizationOptions {
            // Flate preserves decoded samples without another lossy JPEG encoding.
            format: Some(ImageCompression::Flate),
            quality: None,
            // The library default imposes a 2 MB decoded-pixel budget and resizes.
            // Preserve the complete source resolution, regardless of page fit.
            max_image_size: None,
            auto_optimize: Some(false),
            convert_to_greyscale: Some(false),
            dither_greyscale: Some(false),
        }),
    }
}

fn decode_oriented_image(bytes: &[u8]) -> Result<RawImage, String> {
    let mut decoder = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .into_decoder()
        .map_err(|error| error.to_string())?;
    // Missing or malformed optional metadata must not prevent decoding the pixels.
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    // Normalize pixels before any page geometry is calculated, including mirrored orientations.
    image.apply_orientation(orientation);
    let (width, height) = (image.width() as usize, image.height() as usize);

    // printpdf's DynamicImage adapter only accepts 8-bit images. Preserve the higher
    // bit depths supported by its byte decoder for PNG/TIFF inputs as well.
    let (pixels, data_format) = match image {
        DynamicImage::ImageLuma16(buffer) => {
            (RawImageData::U16(buffer.into_raw()), RawImageFormat::R16)
        }
        DynamicImage::ImageLumaA16(buffer) => {
            (RawImageData::U16(buffer.into_raw()), RawImageFormat::RG16)
        }
        DynamicImage::ImageRgb16(buffer) => {
            (RawImageData::U16(buffer.into_raw()), RawImageFormat::RGB16)
        }
        DynamicImage::ImageRgba16(buffer) => {
            (RawImageData::U16(buffer.into_raw()), RawImageFormat::RGBA16)
        }
        DynamicImage::ImageRgb32F(buffer) => {
            (RawImageData::F32(buffer.into_raw()), RawImageFormat::RGBF32)
        }
        DynamicImage::ImageRgba32F(buffer) => (
            RawImageData::F32(buffer.into_raw()),
            RawImageFormat::RGBAF32,
        ),
        image => return RawImage::from_dynamic_image(image),
    };
    Ok(RawImage {
        pixels,
        width,
        height,
        data_format,
        tag: Vec::new(),
    })
}

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
            let image = decode_oriented_image(&bytes).map_err(|error| {
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
            let native_width = image.width as f32 * 72.0 / IMAGE_LAYOUT_DPI;
            let native_height = image.height as f32 * 72.0 / IMAGE_LAYOUT_DPI;
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
                        dpi: Some(IMAGE_LAYOUT_DPI),
                        ..Default::default()
                    },
                }],
            );
            let pdf = document
                .with_pages(vec![page])
                .save(&preserve_detail_save_options(), &mut warnings);
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

    use super::{decode_oriented_image, ImagePdfEngine};
    use crate::conversion::{detect_format, ConversionEngine, ConversionRequest, EngineOutput};

    fn asymmetric_jpeg() -> Vec<u8> {
        let pixels = image::RgbImage::from_fn(3, 2, |x, y| {
            let value = (x + y * 3) as u8 * 40;
            image::Rgb([value, value, value])
        });
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        jpeg.into_inner()
    }

    fn with_exif(jpeg: &[u8], tiff: &[u8]) -> Vec<u8> {
        let mut result = jpeg[..2].to_vec();
        result.extend_from_slice(&[0xff, 0xe1]);
        result.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        result.extend_from_slice(b"Exif\0\0");
        result.extend_from_slice(tiff);
        result.extend_from_slice(&jpeg[2..]);
        result
    }

    fn oriented_jpeg(jpeg: &[u8], orientation: u8, little_endian: bool) -> Vec<u8> {
        // A TIFF IFD containing a single SHORT Orientation (0x0112) entry.
        let tiff = if little_endian {
            vec![
                b'I',
                b'I',
                42,
                0,
                8,
                0,
                0,
                0,
                1,
                0,
                0x12,
                1,
                3,
                0,
                1,
                0,
                0,
                0,
                orientation,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ]
        } else {
            vec![
                b'M',
                b'M',
                0,
                42,
                0,
                0,
                0,
                8,
                0,
                1,
                1,
                0x12,
                0,
                3,
                0,
                0,
                0,
                1,
                0,
                orientation,
                0,
                0,
                0,
                0,
                0,
                0,
            ]
        };
        with_exif(jpeg, &tiff)
    }

    #[test]
    fn normalizes_all_eight_jpeg_exif_orientations_in_both_byte_orders() {
        let jpeg = asymmetric_jpeg();
        let original = image::load_from_memory(&jpeg).unwrap().to_rgb8().into_raw();
        // Original row-major pixels: A B C / D E F. Each expected ordering is
        // explicit so mirrors and transposes are checked independently of image's API.
        let expected_orders = [
            [0, 1, 2, 3, 4, 5],
            [2, 1, 0, 5, 4, 3],
            [5, 4, 3, 2, 1, 0],
            [3, 4, 5, 0, 1, 2],
            [0, 3, 1, 4, 2, 5],
            [3, 0, 4, 1, 5, 2],
            [5, 2, 4, 1, 3, 0],
            [2, 5, 1, 4, 0, 3],
        ];
        for little_endian in [false, true] {
            for orientation in 1..=8 {
                let decoded =
                    decode_oriented_image(&oriented_jpeg(&jpeg, orientation, little_endian))
                        .unwrap();
                let expected_size = if orientation <= 4 { (3, 2) } else { (2, 3) };
                assert_eq!((decoded.width, decoded.height), expected_size);
                let expected: Vec<u8> = expected_orders[orientation as usize - 1]
                    .iter()
                    .flat_map(|&index| original[index * 3..index * 3 + 3].iter().copied())
                    .collect();
                assert_eq!(
                    decoded.pixels,
                    printpdf::RawImageData::U8(expected),
                    "orientation {orientation}, little_endian {little_endian}"
                );
            }
        }
    }

    #[test]
    fn missing_invalid_and_malformed_exif_leave_pixels_unmodified() {
        let jpeg = asymmetric_jpeg();
        let original = decode_oriented_image(&jpeg).unwrap();
        for bytes in [
            jpeg.clone(),
            oriented_jpeg(&jpeg, 0, true),
            oriented_jpeg(&jpeg, 9, false),
            with_exif(&jpeg, b"II\x2a\0\xff\xff\xff\xff"),
            with_exif(&jpeg, b"MM\0"),
        ] {
            assert_eq!(decode_oriented_image(&bytes).unwrap(), original);
        }
    }

    #[test]
    fn preserves_sixteen_bit_png_pixels() {
        let pixels = image::ImageBuffer::from_pixel(3, 2, image::Rgb([1024u16, 32768, 65535]));
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb16(pixels.clone())
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let decoded = decode_oriented_image(png.get_ref()).unwrap();
        assert_eq!(decoded.data_format, printpdf::RawImageFormat::RGB16);
        assert_eq!(
            decoded.pixels,
            printpdf::RawImageData::U16(pixels.into_raw())
        );
    }

    async fn embedded_image_after_conversion(bytes: &[u8], extension: &str) -> printpdf::RawImage {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join(format!("detail.{extension}"));
        std::fs::write(&source, bytes).unwrap();
        let request = ConversionRequest {
            id: uuid::Uuid::new_v4(),
            source_name: format!("detail.{extension}"),
            detected: detect_format(&source).unwrap(),
            source_path: source,
            output_path: directory.path().join("detail.pdf"),
            work_directory: directory.path().into(),
            source_size: bytes.len() as u64,
        };
        ImagePdfEngine.convert(&request).await.unwrap();
        let pdf = printpdf::PdfDocument::parse(
            &std::fs::read(request.staged_output_path()).unwrap(),
            &printpdf::PdfParseOptions::default(),
            &mut Vec::new(),
        )
        .unwrap();
        pdf.resources
            .xobjects
            .map
            .into_values()
            .find_map(|object| match object {
                printpdf::XObject::Image(image) => Some(image),
                _ => None,
            })
            .unwrap()
    }

    #[tokio::test]
    async fn preserves_full_resolution_and_fine_detail_in_a_4000_by_3000_scan() {
        // 36 MB decoded: well beyond printpdf's implicit 2 MB budget. Single-pixel
        // rules and alternating colors expose both downsampling and lossy encoding.
        let pixels = image::RgbImage::from_fn(4000, 3000, |x, y| {
            image::Rgb([
                if x % 17 == 0 || y % 19 == 0 { 0 } else { 255 },
                ((x + y) % 256) as u8,
                ((x ^ y) % 256) as u8,
            ])
        });
        let expected_pixels = pixels.as_raw().clone();
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let embedded = embedded_image_after_conversion(png.get_ref(), "png").await;
        assert_eq!((embedded.width, embedded.height), (4000, 3000));
        assert_eq!(embedded.data_format, printpdf::RawImageFormat::RGB8);
        let printpdf::RawImageData::U8(actual) = embedded.pixels else {
            panic!("expected 8-bit RGB pixels");
        };
        // Compare slices via a boolean to avoid dumping millions of bytes on failure.
        assert!(
            actual == expected_pixels,
            "embedded fine detail must be pixel-exact"
        );
    }

    #[tokio::test]
    async fn preserves_decoded_jpeg_samples_without_additional_lossy_compression() {
        let pixels = image::RgbImage::from_fn(97, 65, |x, y| {
            image::Rgb([(x * 37) as u8, (y * 53) as u8, (x * y) as u8])
        });
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let expected = decode_oriented_image(jpeg.get_ref()).unwrap();
        let embedded = embedded_image_after_conversion(jpeg.get_ref(), "jpg").await;
        assert_eq!((embedded.width, embedded.height), (97, 65));
        assert_eq!(embedded.data_format, expected.data_format);
        assert!(
            embedded.pixels == expected.pixels,
            "PDF must not add JPEG loss"
        );
    }

    #[tokio::test]
    async fn pdf_page_and_placement_use_oriented_jpeg_dimensions() {
        let directory = tempfile::tempdir().unwrap();
        let jpeg = asymmetric_jpeg();
        for orientation in 1..=8 {
            let source = directory
                .path()
                .join(format!("orientation-{orientation}.jpg"));
            let bytes = oriented_jpeg(&jpeg, orientation, true);
            std::fs::write(&source, &bytes).unwrap();
            let request = ConversionRequest {
                id: uuid::Uuid::new_v4(),
                source_name: "oriented.jpg".into(),
                detected: detect_format(&source).unwrap(),
                source_path: source,
                output_path: directory
                    .path()
                    .join(format!("orientation-{orientation}.pdf")),
                work_directory: directory.path().into(),
                source_size: bytes.len() as u64,
            };
            ImagePdfEngine.convert(&request).await.unwrap();
            let pdf = printpdf::PdfDocument::parse(
                &std::fs::read(request.staged_output_path()).unwrap(),
                &printpdf::PdfParseOptions::default(),
                &mut Vec::new(),
            )
            .unwrap();
            assert_eq!(pdf.pages.len(), 1);
            let page = &pdf.pages[0];
            let (width, height, page_width, page_height) = if orientation <= 4 {
                (3, 2, 297.0, 210.0)
            } else {
                (2, 3, 210.0, 297.0)
            };
            let page_width = printpdf::Mm(page_width).into_pt().0;
            let page_height = printpdf::Mm(page_height).into_pt().0;
            assert!((page.media_box.width.0 - page_width).abs() < 0.01);
            assert!((page.media_box.height.0 - page_height).abs() < 0.01);
            let embedded = pdf
                .resources
                .xobjects
                .map
                .values()
                .find_map(|object| match object {
                    printpdf::XObject::Image(image) => Some(image),
                    _ => None,
                })
                .unwrap();
            assert_eq!((embedded.width, embedded.height), (width, height));
            let matrix = page
                .ops
                .iter()
                .filter_map(|op| match op {
                    printpdf::Op::SetTransformationMatrix { matrix } => Some(matrix.as_array()),
                    _ => None,
                })
                .fold(
                    [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                    printpdf::CurTransMat::combine_matrix,
                );
            let rendered_width = width as f32 * 72.0 / 300.0;
            let rendered_height = height as f32 * 72.0 / 300.0;
            let expected = [
                rendered_width,
                0.0,
                0.0,
                rendered_height,
                (page_width - rendered_width) / 2.0,
                (page_height - rendered_height) / 2.0,
            ];
            for (actual, expected) in matrix.into_iter().zip(expected) {
                assert!(
                    (actual - expected).abs() < 0.01,
                    "orientation {orientation}: {actual} != {expected}"
                );
            }
        }
    }

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
