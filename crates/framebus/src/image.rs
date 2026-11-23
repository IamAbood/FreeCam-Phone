//! NV12 frame helpers shared by the receiver and the virtual camera sources.

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    /// NV12: `width*height` luma bytes, then `width*height/2` interleaved UV.
    pub data: Vec<u8>,
}

impl Frame {
    pub fn new(width: usize, height: usize) -> Frame {
        let mut data = vec![16u8; width * height * 3 / 2];
        data[width * height..].fill(128);
        Frame {
            width,
            height,
            data,
        }
    }

    /// Nearest-neighbour resize. ponytail: nearest only; bilinear if quality complaints.
    pub fn scaled(&self, w: usize, h: usize) -> Frame {
        if (w, h) == (self.width, self.height) {
            return self.clone();
        }
        let mut out = Frame::new(w, h);
        let xs: Vec<usize> = (0..w).map(|x| x * self.width / w).collect();
        for y in 0..h {
            let sy = y * self.height / h;
            let src = &self.data[sy * self.width..];
            let dst = &mut out.data[y * w..(y + 1) * w];
            for (d, &sx) in dst.iter_mut().zip(&xs) {
                *d = src[sx];
            }
        }
        let (sw2, sh2, w2, h2) = (self.width / 2, self.height / 2, w / 2, h / 2);
        let (suv, duv) = (self.width * self.height, w * h);
        for y in 0..h2 {
            let sy = (y * sh2 / h2).min(sh2.saturating_sub(1));
            for x in 0..w2 {
                let sx = (x * sw2 / w2).min(sw2.saturating_sub(1));
                let s = suv + sy * self.width + sx * 2;
                let d = duv + y * w + x * 2;
                out.data[d] = self.data[s];
                out.data[d + 1] = self.data[s + 1];
            }
        }
        out
    }

    pub fn mirror(&mut self) {
        let (w, h) = (self.width, self.height);
        for row in self.data[..w * h].chunks_exact_mut(w) {
            row.reverse();
        }
        for row in self.data[w * h..].chunks_exact_mut(w) {
            row.reverse();
            for uv in row.as_chunks_mut::<2>().0 {
                uv.swap(0, 1);
            }
        }
    }

    pub fn rotate180(&mut self) {
        let wh = self.width * self.height;
        self.data[..wh].reverse();
        let uv = &mut self.data[wh..];
        uv.reverse();
        for p in uv.as_chunks_mut::<2>().0 {
            p.swap(0, 1);
        }
    }

    /// Bottom-up BGR24 (a DIB with positive height, which softcam expects).
    pub fn to_bgr24_dib(&self, out: &mut Vec<u8>) {
        self.convert(out, 3, true, |px, r, g, b| px.copy_from_slice(&[b, g, r]));
    }

    pub fn to_rgba(&self, out: &mut Vec<u8>) {
        self.convert(out, 4, false, |px, r, g, b| {
            px.copy_from_slice(&[r, g, b, 255])
        });
    }

    fn convert(
        &self,
        out: &mut Vec<u8>,
        bpp: usize,
        flip: bool,
        put: impl Fn(&mut [u8], u8, u8, u8),
    ) {
        let (w, h) = (self.width, self.height);
        out.resize(w * h * bpp, 0);
        let (y_plane, uv) = self.data.split_at(w * h);
        for y in 0..h {
            for x in 0..w {
                // BT.601 limited range, integer approximation.
                let c = y_plane[y * w + x] as i32 - 16;
                let i = (y / 2) * w + (x & !1);
                let d = uv[i] as i32 - 128;
                let e = uv[i + 1] as i32 - 128;
                let r = (298 * c + 409 * e + 128) >> 8;
                let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
                let b = (298 * c + 516 * d + 128) >> 8;
                let oy = if flip { h - 1 - y } else { y };
                let o = (oy * w + x) * bpp;
                put(
                    &mut out[o..o + bpp],
                    r.clamp(0, 255) as u8,
                    g.clamp(0, 255) as u8,
                    b.clamp(0, 255) as u8,
                );
            }
        }
    }

    /// The "FreeCam Phone — waiting for phone" card shown when no phone is connected.
    pub fn placeholder(w: usize, h: usize) -> Frame {
        const TEXT: &str = "FreeCam Phone \u{2014} waiting for phone";
        let mut f = Frame::new(w, h);
        f.data[..w * h].fill(40);
        let cols = TEXT.chars().count() * 6;
        let scale = (w * 8 / 10 / cols).max(1);
        let x0 = (w.saturating_sub(cols * scale)) / 2;
        let y0 = (h.saturating_sub(7 * scale)) / 2;
        for (i, ch) in TEXT.chars().enumerate() {
            let g = glyph(ch);
            for (gy, bits) in g.iter().enumerate() {
                for gx in 0..5 {
                    if bits >> (4 - gx) & 1 == 0 {
                        continue;
                    }
                    for dy in 0..scale {
                        let y = y0 + gy * scale + dy;
                        let x = x0 + (i * 6 + gx) * scale;
                        if y < h && x + scale <= w {
                            f.data[y * w + x..y * w + x + scale].fill(235);
                        }
                    }
                }
            }
        }
        f
    }
}

fn glyph(c: char) -> [u8; 7] {
    match c {
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'r' => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
        'e' => [0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E],
        'a' => [0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F],
        'm' => [0x00, 0x00, 0x1A, 0x15, 0x15, 0x11, 0x11],
        'h' => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x11],
        'o' => [0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E],
        'n' => [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11],
        'w' => [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0A],
        'i' => [0x04, 0x00, 0x0C, 0x04, 0x04, 0x04, 0x0E],
        't' => [0x08, 0x08, 0x1C, 0x08, 0x08, 0x09, 0x06],
        'g' => [0x00, 0x0F, 0x11, 0x11, 0x0F, 0x01, 0x0E],
        'f' => [0x06, 0x09, 0x08, 0x1C, 0x08, 0x08, 0x08],
        '\u{2014}' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        _ => [0; 7],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(w: usize, h: usize) -> Frame {
        let mut f = Frame::new(w, h);
        for (i, b) in f.data.iter_mut().enumerate() {
            *b = (i * 7 % 251) as u8;
        }
        f
    }

    #[test]
    fn mirror_and_rotate_are_involutions() {
        let f = ramp(8, 4);
        let mut g = f.clone();
        g.mirror();
        assert_ne!(f, g);
        g.mirror();
        assert_eq!(f, g);
        g.rotate180();
        assert_ne!(f, g);
        g.rotate180();
        assert_eq!(f, g);
    }

    #[test]
    fn mirror_keeps_uv_pairs() {
        let mut f = Frame::new(4, 2);
        f.data[8..12].copy_from_slice(&[1, 2, 3, 4]); // (U,V)=(1,2),(3,4)
        f.mirror();
        assert_eq!(&f.data[8..12], &[3, 4, 1, 2]);
    }

    #[test]
    fn scale_sizes_and_identity() {
        let f = ramp(64, 36);
        assert_eq!(f.scaled(64, 36), f);
        let s = f.scaled(32, 18);
        assert_eq!(s.data.len(), 32 * 18 * 3 / 2);
        assert_eq!(s.data[0], f.data[0]);
        let up = f.scaled(1920, 1080);
        assert_eq!(up.data.len(), 1920 * 1080 * 3 / 2);
    }

    #[test]
    fn placeholder_has_text() {
        let p = Frame::placeholder(1280, 720);
        assert!(p.data[..1280 * 720].contains(&235));
        let mut rgba = Vec::new();
        p.to_rgba(&mut rgba);
        assert_eq!(rgba.len(), 1280 * 720 * 4);
    }
}
