//! Immutable, compact 4:2:0 video frames.
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nv12Matrix {
    Bt601,
    Bt709,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nv12Frame {
    width: u32,
    height: u32,
    bytes: Vec<u8>,
    matrix: Nv12Matrix,
    ramps: Option<[[u8; 256]; 3]>,
}

impl Nv12Frame {
    pub fn new(width: u32, height: u32, bytes: Vec<u8>, matrix: Nv12Matrix, ramps: Option<[[u8; 256]; 3]>) -> Result<Self, &'static str> {
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err("NV12 dimensions must be positive and even");
        }
        if width > 16384 || height > 16384 {
            return Err("NV12 dimensions exceed the supported 16384-pixel bound");
        }
        if u64::from(width)*u64::from(height) > (512u64 << 20)/4 {
            return Err("NV12 raster projection exceeds the supported 512 MiB frame bound");
        }
        let n = (width as usize).checked_mul(height as usize).and_then(|v| v.checked_mul(3)).map(|v| v / 2).ok_or("NV12 frame size overflow")?;
        if n > 1_073_741_824 {
            return Err("NV12 frame exceeds the 1 GiB source bound");
        }
        if bytes.len() != n {
            return Err("NV12 byte length does not match dimensions");
        }
        Ok(Self { width, height, bytes, matrix, ramps })
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn matrix(&self) -> Nv12Matrix {
        self.matrix
    }
    pub fn ramps(&self) -> Option<&[[u8; 256]; 3]> {
        self.ramps.as_ref()
    }
    pub fn rgba8(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0; 4];
        }
        let w = self.width as usize;
        let x = x as usize;
        let y = y as usize;
        let yy = self.bytes[y * w + x] as f64;
        let uv = w * self.height as usize + (y / 2) * w + (x / 2) * 2;
        let u = self.bytes[uv] as f64;
        let v = self.bytes[uv + 1] as f64;
        let (kr, kb) = match self.matrix {
            Nv12Matrix::Bt601 => (0.299, 0.114),
            Nv12Matrix::Bt709 => (0.2126, 0.0722),
        };
        let yy = (yy - 16.0) * (255.0 / 219.0);
        let cb = (u - 128.0) * (255.0 / 224.0);
        let cr = (v - 128.0) * (255.0 / 224.0);
        let kg = 1.0 - kr - kb;
        let q = |z: f64| ((z + 0.5).floor() as i32).clamp(0, 255) as u8;
        let mut out =
            [q(yy + 2.0 * (1.0 - kr) * cr), q(yy - 2.0 * kb * (1.0 - kb) / kg * cb - 2.0 * kr * (1.0 - kr) / kg * cr), q(yy + 2.0 * (1.0 - kb) * cb), 255];
        for c in 0..3 {
            out[c] = self.ramps.as_ref().map_or(out[c], |r| r[c][out[c] as usize]);
        }
        out
    }
}

pub struct Nv12View<'a> {
    pub frame: &'a Arc<Nv12Frame>,
    pub origin: [i32; 2],
}
