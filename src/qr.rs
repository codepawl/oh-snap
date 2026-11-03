//! Reads a QR code out of an image. Pure Rust, the same on every OS.

use crate::output::Image;

/// The payload of the first QR code that decodes.
pub fn read_qr_code(image: &Image) -> Option<String> {
    let width = image.width as usize;
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(width, image.height as usize, |x, y| {
        let pixel = &image.bgra[(y * width + x) * 4..][..3];
        ((u32::from(pixel[0]) * 29 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 77) >> 8) as u8
    });
    prepared
        .detect_grids()
        .iter()
        .find_map(|grid| grid.decode().ok().map(|(_, payload)| payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qr_fixture() -> Image {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/qr.png");
        crate::image_file::load_png(&path).unwrap()
    }

    #[test]
    fn reads_a_qr_code() {
        assert_eq!(read_qr_code(&qr_fixture()).as_deref(), Some("https://github.com/codepawl/oh-snap"));
    }

    #[test]
    fn a_plain_image_has_no_qr_code() {
        let image = Image {
            width: 64,
            height: 64,
            bgra: vec![200; 64 * 64 * 4],
        };
        assert_eq!(read_qr_code(&image), None);
    }
}
