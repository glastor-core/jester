use image::ImageFormat;
use std::io::Cursor;

/// Decodes a PNG or JPEG image in pure Rust and re-encodes it as lossless WebP.
/// Returns `Some(webp_bytes)` only when the resulting WebP is strictly smaller than the input.
pub fn optimize_image_to_webp(bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::WebP).ok()?;
    let webp = out.into_inner();
    (webp.len() < bytes.len()).then_some(webp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn converts_png_to_smaller_valid_webp_in_pure_rust() {
        let mut img = RgbaImage::new(64, 64);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            let r = u8::try_from((x * 4) % 256).unwrap_or(0);
            let g = u8::try_from((y * 4) % 256).unwrap_or(0);
            *pixel = Rgba([r, g, 180, 255]);
        }
        let mut png_buf = Cursor::new(Vec::new());
        img.write_to(&mut png_buf, ImageFormat::Png)
            .expect("encode test png");
        let png_bytes = png_buf.into_inner();

        let webp = optimize_image_to_webp(&png_bytes)
            .expect("gradient PNG should compress smaller as WebP");
        assert!(webp.len() < png_bytes.len());
        assert_eq!(&webp[..4], b"RIFF");
        assert_eq!(&webp[8..12], b"WEBP");
    }
}
