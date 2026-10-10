//! Optional native media contract checks. Run serially with `PHOTOSTORE_MEDIA_SMOKE=1`.

use std::{env, fs, path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

use photostore_color::PixelFormat;
use photostore_geom::Rect;
use photostore_engine::media_backend::{self, Rational, Result, VideoFormat, VideoWriterConfig};
use photostore_raster::Rgba8Image;

fn fixture_path(name: &str) -> PathBuf {
    env::temp_dir().join(format!("photostore-media-{name}-{}-{}.mp4", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()))
}

#[test]
fn media_backend_smoke_contracts() -> Result<()> {
    if env::var("PHOTOSTORE_MEDIA_SMOKE").ok().as_deref() != Some("1") { return Ok(()); }
    let path = fixture_path("two-frame");
    let cfg = VideoWriterConfig { width: 8, height: 8, fps: Rational::new(10, 1), format: VideoFormat::Mp4H264, duration_frames: Some(2), overwrite: true };
    media_backend::write_video_from_frames(&path, &cfg, &[], |i| {
        let mut image = Rgba8Image::new(8, 8);
        let px = if i == 0 { [255, 0, 0, 255] } else { [0, 255, 0, 255] };
        for chunk in image.pixels.chunks_exact_mut(4) { chunk.copy_from_slice(&px); }
        Ok(image)
    }, None)?;
    let info = media_backend::probe(&path)?;
    assert_eq!((info.width, info.height), (8, 8));
    let frames = media_backend::decode_sequence(&path, PixelFormat::RGBA8, 512 * 1024 * 1024)?;
    assert_eq!(frames.len(), 2);
    let mut samples = [[0u8; 4]; 1];
    frames[0].read_rgba8_into(Rect::from_xywh(0, 0, 1, 1), &mut samples);
    assert!(samples[0][0] > 180 && samples[0][0] > samples[0][1].saturating_add(60));
    frames[1].read_rgba8_into(Rect::from_xywh(0, 0, 1, 1), &mut samples);
    assert!(samples[0][1] > 140 && samples[0][1] > samples[0][0].saturating_add(30));
    let range = media_backend::decode_sequence_range(&path, 1, 1, PixelFormat::RGBA8, 512 * 1024 * 1024, |_, _| true)?;
    range[0].read_rgba8_into(Rect::from_xywh(0, 0, 1, 1), &mut samples);
    assert!(samples[0][1] > 140 && samples[0][1] > samples[0][0].saturating_add(30));
    let existing = media_backend::VideoWriterConfig { overwrite: false, ..cfg };
    assert!(media_backend::write_video_from_frames(&path, &existing, &[], |_| Ok(Rgba8Image::new(8, 8)), None).is_err());
    let cancelled_path = fixture_path("cancelled");
    let mut cancel = |_: u64, _: Option<u64>| false;
    let cancelled = media_backend::write_video_from_frames(&cancelled_path, &cfg, &[], |_| Ok(Rgba8Image::new(8, 8)), Some(&mut cancel));
    assert!(matches!(cancelled, Err(media_backend::MediaBackendError::Cancelled)));
    assert!(!cancelled_path.exists());
    let _ = fs::remove_file(&path);
    for (name, format) in [("alpha-mov", VideoFormat::MovProResAlpha), ("alpha-webm", VideoFormat::WebmVp9Alpha)] {
        let alpha_path = fixture_path(name);
        let alpha_cfg = VideoWriterConfig { width: 8, height: 8, fps: Rational::new(10, 1), format, duration_frames: Some(2), overwrite: true };
        media_backend::write_video_from_frames(&alpha_path, &alpha_cfg, &[], |_| {
            let mut image = Rgba8Image::new(8, 8);
            for px in image.pixels.chunks_exact_mut(4) { px.copy_from_slice(&[255, 0, 0, 128]); }
            Ok(image)
        }, None)?;
        let decoded = media_backend::decode_sequence(&alpha_path, PixelFormat::RGBA8, 512 * 1024 * 1024)?;
        let mut rgba = [[0u8; 4]; 1];
        decoded[0].read_rgba8_into(Rect::from_xywh(0, 0, 1, 1), &mut rgba);
        assert!((i16::from(rgba[0][3]) - 128).unsigned_abs() <= 3);
        let _ = fs::remove_file(alpha_path);
    }
    Ok(())
}

