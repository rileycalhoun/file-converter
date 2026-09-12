use async_trait::async_trait;
use image::{DynamicImage, ImageDecoder, ImageReader};
use printpdf::{
    ImageCompression, ImageOptimizationOptions, Mm, Op, PdfDocument, PdfPage, PdfSaveOptions, Pt,
    RawImage, RawImageData, RawImageFormat, XObjectTransform,
};
use std::io::Read;

use crate::conversion::{
    ConversionEngine, ConversionError, ConversionRequest, ConversionResult, DetectedFormat,
    EngineOutput, InputFamily,
};

pub struct ImagePdfEngine;

// Physical placement only: fitting to A4 changes the PDF transform, never the pixels.
const IMAGE_LAYOUT_DPI: f32 = 300.0;
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DECODED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 40_000_000;

fn read_image_source(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let oversized = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Image exceeds the 64 MiB source limit. Export a smaller image and try again.",
        )
    };
    if file.metadata()?.len() > MAX_SOURCE_BYTES {
        return Err(oversized());
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(oversized());
    }
    Ok(bytes)
}

fn check_pixel_limit(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        return Err("Image exceeds the 40 megapixel limit or has invalid dimensions. Export a smaller image and try again.".into());
    }
    Ok(())
}

// Direct DCT embedding is safe for this narrow baseline JFIF subset. EXIF transforms,
// ICC profiles, Adobe/CMYK color interpretation, and other JPEG variants use the decoder.
fn direct_jpeg_info(bytes: &[u8]) -> Option<(u32, u32, bool)> {
    if !bytes.starts_with(&[0xff, 0xd8]) || !bytes.ends_with(&[0xff, 0xd9]) {
        return None;
    }
    let mut offset = 2;
    let mut jfif = false;
    let mut frame = None;
    while offset < bytes.len() {
        if *bytes.get(offset)? != 0xff {
            return None;
        }
        while *bytes.get(offset)? == 0xff {
            offset += 1;
        }
        let marker = *bytes.get(offset)?;
        offset += 1;
        let length = u16::from_be_bytes([*bytes.get(offset)?, *bytes.get(offset + 1)?]) as usize;
        if length < 2 {
            return None;
        }
        let segment = bytes.get(offset + 2..offset.checked_add(length)?)?;
        offset += length;
        match marker {
            0xe0 => jfif |= segment.starts_with(b"JFIF\0"),
            0xe1 if segment.starts_with(b"Exif\0\0") => {
                if image::metadata::Orientation::from_exif_chunk(&segment[6..])?
                    != image::metadata::Orientation::NoTransforms
                {
                    return None;
                }
            }
            0xe2 | 0xee => return None,
            0xc0 => {
                if frame.is_some() || *segment.first()? != 8 {
                    return None;
                }
                let height = u16::from_be_bytes([*segment.get(1)?, *segment.get(2)?]) as u32;
                let width = u16::from_be_bytes([*segment.get(3)?, *segment.get(4)?]) as u32;
                let components = *segment.get(5)? as usize;
                if !matches!(components, 1 | 3) || segment.len() != 6 + 3 * components {
                    return None;
                }
                for index in 0..components {
                    if segment[6 + index * 3] != (index + 1) as u8 {
                        return None;
                    }
                }
                frame = Some((width, height, components == 1));
            }
            0xda => {
                let (_, _, grayscale) = frame?;
                let components = *segment.first()? as usize;
                if !jfif
                    || components != if grayscale { 1 } else { 3 }
                    || segment.len() != 4 + 2 * components
                    || offset >= bytes.len() - 2
                    || segment[segment.len() - 3..] != [0, 63, 0]
                {
                    return None;
                }
                for index in 0..components {
                    if segment[1 + index * 2] != (index + 1) as u8 {
                        return None;
                    }
                }
                // Require a single complete baseline scan. Other scans or metadata
                // after SOS must go through the decoder rather than bypassing checks.
                while offset < bytes.len() {
                    if bytes[offset] != 0xff {
                        offset += 1;
                        continue;
                    }
                    while *bytes.get(offset)? == 0xff {
                        offset += 1;
                    }
                    let scan_marker = *bytes.get(offset)?;
                    offset += 1;
                    match scan_marker {
                        0 | 0xd0..=0xd7 => {}
                        0xd9 if offset == bytes.len() => return frame,
                        _ => return None,
                    }
                }
                return None;
            }
            // Standard baseline metadata, quantization/Huffman tables, restart interval.
            0xdb | 0xc4 | 0xdd | 0xfe | 0xe1 | 0xe3..=0xed | 0xef => {}
            _ => return None,
        }
    }
    None
}

fn prepare_image(bytes: Vec<u8>) -> Result<(usize, usize, printpdf::XObject), String> {
    if let Some((width, height, grayscale)) = direct_jpeg_info(&bytes) {
        check_pixel_limit(width, height)?;
        // Validate JPEG headers with the maintained decoder too, without allocating
        // the full decoded raster. The original entropy stream remains untouched.
        image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(&bytes))
            .map_err(|error| error.to_string())?;
        use printpdf::DictItem::{Int, Name};
        let external = printpdf::ExternalXObject {
            stream: printpdf::ExternalStream {
                dict: [
                    ("Type".into(), Name(b"XObject".to_vec())),
                    ("Subtype".into(), Name(b"Image".to_vec())),
                    ("Width".into(), Int(width.into())),
                    ("Height".into(), Int(height.into())),
                    ("BitsPerComponent".into(), Int(8)),
                    (
                        "ColorSpace".into(),
                        Name(if grayscale {
                            b"DeviceGray".to_vec()
                        } else {
                            b"DeviceRGB".to_vec()
                        }),
                    ),
                    ("Filter".into(), Name(b"DCTDecode".to_vec())),
                ]
                .into(),
                content: bytes,
                compress: false,
            },
            width: Some(printpdf::Px(width as usize)),
            height: Some(printpdf::Px(height as usize)),
            dpi: Some(IMAGE_LAYOUT_DPI),
        };
        Ok((
            width as usize,
            height as usize,
            printpdf::XObject::External(external),
        ))
    } else {
        let image = decode_oriented_image(&bytes)?;
        Ok((image.width, image.height, printpdf::XObject::Image(image)))
    }
}

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
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let (width, height) = decoder.dimensions();
    check_pixel_limit(width, height)?;
    if decoder.total_bytes() > MAX_DECODED_BYTES {
        return Err(
            "Image exceeds the 64 MiB decoded-pixel limit. Export a smaller image and try again."
                .into(),
        );
    }
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
                read_image_source(&source_path).map_err(|source| ConversionError::ReadSource {
                    path: source_path.clone(),
                    source,
                })?;
            let mut warnings = Vec::new();
            let (image_width, image_height, image) = prepare_image(bytes).map_err(|error| {
                ConversionError::ConversionFailed(format!(
                    "The image could not be decoded: {error}"
                ))
            })?;

            let (page_width, page_height) = if image_width > image_height {
                (Mm(297.0), Mm(210.0))
            } else {
                (Mm(210.0), Mm(297.0))
            };
            let margin_pt = Mm(10.0).into_pt().0;
            let available_width = page_width.into_pt().0 - margin_pt * 2.0;
            let available_height = page_height.into_pt().0 - margin_pt * 2.0;
            let native_width = image_width as f32 * 72.0 / IMAGE_LAYOUT_DPI;
            let native_height = image_height as f32 * 72.0 / IMAGE_LAYOUT_DPI;
            let scale = (available_width / native_width)
                .min(available_height / native_height)
                .min(1.0);
            let rendered_width = native_width * scale;
            let rendered_height = native_height * scale;

            let mut document = PdfDocument::new(&source_name);
            // Move the owned stream/pixels into the document; add_image would clone them.
            let image_id = printpdf::XObjectId::new();
            document
                .resources
                .xobjects
                .map
                .insert(image_id.clone(), image);
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

    use super::{
        decode_oriented_image, direct_jpeg_info, prepare_image, read_image_source, ImagePdfEngine,
    };
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

    #[test]
    fn embeds_compatible_jpeg_as_original_dct_stream_without_copying_its_buffer() {
        let jpeg = asymmetric_jpeg();
        let expected = jpeg.clone();
        let source_pointer = jpeg.as_ptr();
        let (width, height, object) = prepare_image(jpeg).unwrap();
        assert_eq!((width, height), (3, 2));
        let printpdf::XObject::External(external) = &object else {
            panic!("compatible baseline JFIF should avoid pixel decoding");
        };
        assert_eq!(external.stream.content.as_ptr(), source_pointer);
        assert_eq!(external.stream.content, expected);
        let mut document = printpdf::PdfDocument::new("direct JPEG");
        let id = printpdf::XObjectId::new();
        document.resources.xobjects.map.insert(id.clone(), object);
        document.pages.push(printpdf::PdfPage::new(
            printpdf::Mm(210.0),
            printpdf::Mm(297.0),
            vec![printpdf::Op::UseXobject {
                id,
                transform: printpdf::XObjectTransform::default(),
            }],
        ));
        let pdf =
            document.to_lopdf_document(&super::preserve_detail_save_options(), &mut Vec::new());
        let stream = pdf
            .objects
            .values()
            .filter_map(|object| object.as_stream().ok())
            .find(|stream| {
                stream
                    .dict
                    .get(b"Subtype")
                    .and_then(|value| value.as_name())
                    .ok()
                    == Some(b"Image".as_slice())
            })
            .unwrap();
        assert_eq!(
            stream.dict.get(b"Filter").unwrap().as_name().unwrap(),
            b"DCTDecode"
        );
        assert_eq!(stream.content, expected);
    }

    #[test]
    fn rotated_or_color_profile_jpegs_use_the_normalizing_decoder() {
        let jpeg = asymmetric_jpeg();
        for orientation in 2..=8 {
            let bytes = oriented_jpeg(&jpeg, orientation, true);
            assert!(direct_jpeg_info(&bytes).is_none());
            assert!(matches!(
                prepare_image(bytes).unwrap().2,
                printpdf::XObject::Image(_)
            ));
        }
        for marker in [0xe2, 0xee] {
            let mut bytes = jpeg[..2].to_vec();
            bytes.extend_from_slice(&[0xff, marker, 0, 2]);
            bytes.extend_from_slice(&jpeg[2..]);
            assert!(direct_jpeg_info(&bytes).is_none());
        }
        for bytes in [
            &jpeg[..jpeg.len() - 2],
            &[0xff, 0xd8, 0xff, 0xe0, 0, 1, 0xff, 0xd9][..],
        ] {
            assert!(direct_jpeg_info(bytes).is_none());
        }
    }

    #[test]
    fn rejects_oversized_source_and_header_dimensions_before_pixel_allocation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("oversized.jpg");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(super::MAX_SOURCE_BYTES + 1)
            .unwrap();
        assert!(read_image_source(&path)
            .unwrap_err()
            .to_string()
            .contains("64 MiB source limit"));
        let mut jpeg = asymmetric_jpeg();
        let frame = jpeg
            .windows(2)
            .position(|bytes| bytes == [0xff, 0xc0])
            .unwrap();
        jpeg[frame + 5..frame + 7].copy_from_slice(&6000u16.to_be_bytes());
        jpeg[frame + 7..frame + 9].copy_from_slice(&8000u16.to_be_bytes());
        assert!(prepare_image(jpeg.clone())
            .unwrap_err()
            .contains("40 megapixel"));
        jpeg[frame + 5..frame + 7].copy_from_slice(&5000u16.to_be_bytes());
        jpeg[frame + 7..frame + 9].copy_from_slice(&5000u16.to_be_bytes());
        let rotated = oriented_jpeg(&jpeg, 6, true);
        assert!(prepare_image(rotated)
            .unwrap_err()
            .to_lowercase()
            .contains("limit"));
    }

    #[tokio::test]
    async fn preserves_transparency_on_the_decode_path() {
        let pixels = image::RgbaImage::from_fn(7, 5, |x, y| {
            image::Rgba([(x * 35) as u8, (y * 50) as u8, 100, (x * y * 9) as u8])
        });
        let expected = pixels.as_raw().clone();
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let embedded = embedded_image_after_conversion(png.get_ref(), "png").await;
        assert_eq!((embedded.width, embedded.height), (7, 5));
        assert_eq!(embedded.data_format, printpdf::RawImageFormat::RGBA8);
        assert_eq!(embedded.pixels, printpdf::RawImageData::U8(expected));
    }

    #[test]
    #[ignore = "manual 12-megapixel preparation timing; no timing threshold"]
    fn measure_large_jpeg_preparation() {
        let pixels = image::RgbImage::from_fn(4000, 3000, |x, y| {
            image::Rgb([x as u8, y as u8, (x ^ y) as u8])
        });
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let bytes = jpeg.into_inner();
        let encoded_size = bytes.len();
        let input = bytes.clone();
        let start = std::time::Instant::now();
        let direct = prepare_image(input).unwrap();
        let direct_time = start.elapsed();
        assert!(matches!(direct.2, printpdf::XObject::External(_)));
        drop(direct);
        let start = std::time::Instant::now();
        let decoded = decode_oriented_image(&bytes).unwrap();
        let decoded_time = start.elapsed();
        assert_eq!((decoded.width, decoded.height), (4000, 3000));
        println!("JPEG preparation: direct={direct_time:?}, decoded={decoded_time:?}, encoded={encoded_size} bytes, avoided raster=36000000 bytes");
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
