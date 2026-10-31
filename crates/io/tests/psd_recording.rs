use std::sync::Arc;

use photostore_color::{Color, ColorMode, SampleType};
use photostore_doc::{Document, Recording, RecordingFrame, RecordingQuality, RecordingResolution, RecordingSettings};
use photostore_geom::Size;
use photostore_io::{ExportOptions, export, import};
use photostore_psd::{PsdFile, resources::ImageResource};

fn frame() -> Vec<u8> {
    let image = photostore_codecs::Image::from_u8(1920, 1080, photostore_codecs::ChannelLayout::Rgb, vec![32, 64, 128].repeat(1920 * 1080)).unwrap();
    photostore_codecs::encode(&image, photostore_codecs::Format::Jpeg, &Default::default()).unwrap()
}

fn recording(enabled: bool, frames: Vec<RecordingFrame>, error: Option<&str>) -> Recording {
    Recording {
        settings: RecordingSettings { resolution: RecordingResolution::Hd1080, quality: RecordingQuality::Studio, hevc: true },
        enabled,
        frames: Arc::new(frames),
        error: error.map(str::to_owned),
    }
}

fn bytes(doc: &Document) -> Vec<u8> {
    export(doc, "x.psd", &ExportOptions::default()).unwrap().bytes
}

#[test]
fn recording_and_private_layer_flags_roundtrip_through_psd_bytes() {
    let mut doc = Document::with_background("x", Size::new(16, 16), ColorMode::Rgb, SampleType::U8, Color::BLACK);
    doc.layers[0].private_recording = true;
    let raw = frame();
    for (enabled, error) in [(true, None), (false, Some("paused at limit"))] {
        for extension in ["psd", "psb"] {
            doc.recording = Some(recording(enabled, vec![RecordingFrame { image: Arc::new(raw.clone()), action: "Brush stroke".into() }], error));
            let saved = export(&doc, extension, &ExportOptions::default()).unwrap();
            let psd = PsdFile::from_bytes(&saved.bytes).unwrap();
            let back = import(&format!("x.{extension}"), &psd.to_bytes().unwrap()).unwrap().document;
            let restored = back.recording.unwrap();
            assert_eq!(restored.enabled, enabled);
            assert_eq!(restored.settings.quality, RecordingQuality::Studio);
            assert!(restored.settings.hevc);
            assert_eq!(restored.error.as_deref(), error);
            assert_eq!(restored.frames[0].action, "Brush stroke");
            assert_eq!(restored.frames[0].image.as_slice(), raw.as_slice());
            assert!(back.layers[0].private_recording);
        }
    }
}

#[test]
fn disabled_empty_purged_and_foreign_same_id_resources_are_handled() {
    let mut doc = Document::with_background("x", Size::new(16, 16), ColorMode::Rgb, SampleType::U8, Color::BLACK);
    doc.recording = Some(recording(false, Vec::new(), Some("paused at limit")));
    let recorded_file = PsdFile::from_bytes(&bytes(&doc)).unwrap();
    assert!(recorded_file.resource(4001).is_some());
    doc.recording = None;
    assert!(PsdFile::from_bytes(&bytes(&doc)).unwrap().resource(4001).is_none());

    let stale = recorded_file.resource(4001).unwrap();
    let mut stale_doc = Document::with_background("stale", Size::new(16, 16), ColorMode::Rgb, SampleType::U8, Color::BLACK);
    stale_doc.metadata.psd_resources.push((4001, "PhotoStore.TimeLapse".into(), Arc::new(stale.data.clone())));
    assert!(PsdFile::from_bytes(&bytes(&stale_doc)).unwrap().resource(4001).is_none());

    let mut foreign = PsdFile::from_bytes(&bytes(&doc)).unwrap();
    let mut resource = ImageResource::new(4001, b"foreign-plugin".to_vec());
    resource.name = b"Other.Plugin".to_vec();
    foreign.resources.push(resource);
    let reopened = import("foreign.psd", &foreign.to_bytes().unwrap()).unwrap().document;
    let saved = PsdFile::from_bytes(&bytes(&reopened)).unwrap();
    assert!(saved.resources.iter().any(|r| r.id == 4001 && r.name == b"Other.Plugin" && r.data == b"foreign-plugin"));
    assert!(reopened.recording.is_none());
}

#[test]
fn private_layer_flags_persist_even_when_recording_is_absent() {
    let mut doc = Document::with_background("x", Size::new(16, 16), ColorMode::Rgb, SampleType::U8, Color::BLACK);
    doc.layers[0].private_recording = true;
    let reopened = import("x.psd", &bytes(&doc)).unwrap().document;
    assert!(reopened.recording.is_none());
    assert!(reopened.layers[0].private_recording);
}

#[test]
fn invalid_recording_frame_is_rejected_by_psd_export() {
    let mut doc = Document::with_background("x", Size::new(16, 16), ColorMode::Rgb, SampleType::U8, Color::BLACK);
    doc.recording = Some(recording(true, vec![RecordingFrame { image: Arc::new(vec![1, 2, 3]), action: "bad frame".into() }], None));
    assert!(export(&doc, "x.psd", &ExportOptions::default()).is_err());
}

#[test]
fn malformed_and_future_recording_metadata_open_artwork_and_are_preserved() {
    let doc = Document::with_background("x", Size::new(16, 16), ColorMode::Rgb, SampleType::U8, Color::BLACK);
    for (payload, expected) in [(b"PSTL\0\x01bad".to_vec(), "invalid"), (b"PSTL\0\x02future".to_vec(), "newer version")] {
        let mut file = PsdFile::from_bytes(&bytes(&doc)).unwrap();
        let mut resource = ImageResource::new(4001, payload.clone());
        resource.name = b"PhotoStore.TimeLapse".to_vec();
        file.resources.push(resource);
        let imported = import("x.psd", &file.to_bytes().unwrap()).unwrap();
        assert!(imported.document.recording.is_none());
        assert!(imported.warnings.iter().any(|w| w.contains(expected)));
        let saved = PsdFile::from_bytes(&bytes(&imported.document)).unwrap();
        assert!(saved.resources.iter().any(|r| r.id == 4001 && r.name == b"PhotoStore.TimeLapse" && r.data == payload));
    }
}
