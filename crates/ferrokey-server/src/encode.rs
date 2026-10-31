//! Turns captured frames into JPEG images sized for the phone.

use ferrokey_input::screen::Frame;
use jpeg_encoder::{ColorType, Encoder};

/// Frames are never sent larger than this on their long side, whatever the
/// phone asks for, to keep bandwidth in check.
const MAX_LONG_SIDE: u32 = 1920;
/// A fair trade between size and legible text.
const QUALITY: u8 = 70;

/// Reusable buffers for encoding one client's frames.
#[derive(Default)]
pub struct FrameEncoder {
    rgb: Vec<u8>,
    sums: Vec<u32>,
    pub jpeg: Vec<u8>,
}

impl FrameEncoder {
    /// Scales `frame` to fit `max_width` by `max_height` (in either
    /// orientation) and encodes it into [`FrameEncoder::jpeg`], returning
    /// the encoded size.
    pub fn encode(
        &mut self,
        frame: &Frame,
        max_width: u16,
        max_height: u16,
    ) -> Result<(u16, u16), jpeg_encoder::EncodingError> {
        let (width, height) = fit(frame.width, frame.height, max_width, max_height);
        downscale(frame, width, height, &mut self.rgb, &mut self.sums);
        self.jpeg.clear();
        // `fit` keeps both sides within MAX_LONG_SIDE, so they fit in u16.
        let (width, height) = (width as u16, height as u16);
        Encoder::new(&mut self.jpeg, QUALITY).encode(&self.rgb, width, height, ColorType::Rgb)?;
        Ok((width, height))
    }
}

/// The largest size with the frame's aspect ratio that fits in the box,
/// turned to match the frame's orientation. Never scales up.
fn fit(width: u32, height: u32, max_width: u16, max_height: u16) -> (u32, u32) {
    let (mut box_w, mut box_h) = (u32::from(max_width), u32::from(max_height));
    if (width >= height) != (box_w >= box_h) {
        (box_w, box_h) = (box_h, box_w);
    }
    let box_w = box_w.clamp(1, MAX_LONG_SIDE);
    let box_h = box_h.clamp(1, MAX_LONG_SIDE);
    let (width, height) = (width.max(1), height.max(1));
    if width <= box_w && height <= box_h {
        return (width, height);
    }
    // Compare width/box_w with height/box_h without floating point.
    if u64::from(width) * u64::from(box_h) >= u64::from(height) * u64::from(box_w) {
        let scaled = u64::from(height) * u64::from(box_w) / u64::from(width);
        (box_w, (scaled as u32).max(1))
    } else {
        let scaled = u64::from(width) * u64::from(box_h) / u64::from(height);
        ((scaled as u32).max(1), box_h)
    }
}

/// Shrinks `frame` to `width` by `height` RGB pixels by averaging the
/// source pixels each output pixel covers, which keeps text readable
/// rather than dropping whole rows and columns.
fn downscale(frame: &Frame, width: u32, height: u32, rgb: &mut Vec<u8>, sums: &mut Vec<u32>) {
    let [r, g, b] = frame.channel_offsets();
    let (src_w, src_h) = (frame.width as usize, frame.height as usize);
    let (width, height) = (width as usize, height as usize);
    rgb.clear();
    rgb.reserve(width * height * 3);

    // Source column range covered by each output column.
    let columns: Vec<(usize, usize)> = (0..width).map(|x| span(x, width, src_w)).collect();
    for y in 0..height {
        let (top, bottom) = span(y, height, src_h);
        sums.clear();
        sums.resize(width * 3, 0);
        // The last row may lack its padding.
        let rows = &frame.data[top * frame.stride..(bottom * frame.stride).min(frame.data.len())];
        for row in rows.chunks(frame.stride) {
            for (x, &(left, right)) in columns.iter().enumerate() {
                let sum = &mut sums[x * 3..x * 3 + 3];
                for pixel in row[left * 4..right * 4].chunks_exact(4) {
                    sum[0] += u32::from(pixel[r]);
                    sum[1] += u32::from(pixel[g]);
                    sum[2] += u32::from(pixel[b]);
                }
            }
        }
        for (x, &(left, right)) in columns.iter().enumerate() {
            let count = ((right - left) * (bottom - top)) as u32;
            rgb.extend(
                sums[x * 3..x * 3 + 3]
                    .iter()
                    .map(|&sum| ((sum + count / 2) / count) as u8),
            );
        }
    }
}

/// The source range `[start, end)` that output index `i` of `out` covers in
/// a source of length `src`; never empty.
fn span(i: usize, out: usize, src: usize) -> (usize, usize) {
    let start = i * src / out;
    let end = ((i + 1) * src / out).max(start + 1);
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrokey_input::screen::PixelFormat;

    #[test]
    fn fits_in_either_orientation_without_upscaling() {
        assert_eq!(fit(2560, 1440, 1280, 720), (1280, 720));
        assert_eq!(fit(2560, 1440, 720, 1280), (1280, 720));
        assert_eq!(fit(2560, 1600, 1280, 720), (1152, 720));
        assert_eq!(fit(800, 600, 2400, 1080), (800, 600));
        assert_eq!(fit(1080, 1920, 1280, 720), (720, 1280));
        assert_eq!(fit(7680, 4320, 9999, 9999), (1920, 1080));
        assert_eq!(fit(100, 1, 10, 10), (10, 1));
    }

    #[test]
    fn averages_pixels_and_swaps_channels() {
        // 4x2 BGRx frame with a padded stride: left half red, right half
        // alternating black and white.
        let stride = 4 * 4 + 8;
        let mut data = vec![0u8; stride * 2];
        for y in 0..2 {
            for x in 0..4 {
                let pixel = &mut data[y * stride + x * 4..][..4];
                let bgr = match (x, (x + y) % 2) {
                    (0 | 1, _) => [0, 0, 255],
                    (_, 0) => [0, 0, 0],
                    _ => [255, 255, 255],
                };
                pixel[..3].copy_from_slice(&bgr);
            }
        }
        let frame = Frame {
            width: 4,
            height: 2,
            stride,
            format: PixelFormat::Bgrx,
            data,
        };
        let (mut rgb, mut sums) = (Vec::new(), Vec::new());
        downscale(&frame, 2, 1, &mut rgb, &mut sums);
        assert_eq!(rgb, [255, 0, 0, 128, 128, 128]);

        let mut encoder = FrameEncoder::default();
        assert_eq!(encoder.encode(&frame, 10, 10).unwrap(), (4, 2));
        assert_eq!(&encoder.jpeg[..2], [0xff, 0xd8]);
    }
}
