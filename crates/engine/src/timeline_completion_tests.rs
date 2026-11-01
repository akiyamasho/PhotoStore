//! Focused source/timing/persistence boundaries changed for issue #23. Uses disposable synthetic
//! media only; it neither opens an audio device nor claims native UI acceptance.
use crate::Session;
use photostore_doc::{Document, Layer, LayerContent, MovieBacking, SampleType, Size, Timeline, VideoData, VideoSource};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

pub(crate) fn fixture() -> Option<PathBuf> {
    static FIXTURE: OnceLock<Option<PathBuf>> = OnceLock::new();
    FIXTURE
        .get_or_init(|| {
            if std::env::var_os("PHOTOSTORE_MEDIA_CACHE_DIR").is_none() {
                eprintln!("Skipping external-media fixture: set PHOTOSTORE_MEDIA_CACHE_DIR to a disposable test cache");
                return None;
            }
            let Some(exe) = crate::media_backend::resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg") else {
                eprintln!("Skipping external media fixture: ffmpeg unavailable");
                return None;
            };
            let path = std::env::temp_dir().join(format!("photostore-timeline-motion-{}.mkv", std::process::id()));
            let status = std::process::Command::new(exe)
                .args([
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=64x64:rate=30:duration=3",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=220:sample_rate=48000:duration=3",
                    "-c:v",
                    "ffv1",
                    "-c:a",
                    "pcm_s16le",
                ])
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
            Some(path)
        })
        .clone()
}
fn frame_hash(doc: &Document) -> blake3::Hash {
    let layer = doc.layers.first().unwrap();
    let surface = layer.surface().unwrap();
    let mut pixels = vec![[0.0; 4]; 64 * 64];
    surface.read_rgba_into(doc.bounds(), &mut pixels);
    let bytes: Vec<_> = pixels.into_iter().flat_map(|p| p.map(|v| (v * 255.0).round() as u8)).collect();
    blake3::hash(&bytes)
}

#[test]
fn scalable_playback_async_motion_and_original_export_preserve_document() {
    let Some(path) = fixture() else { return };
    let info = crate::media_backend::probe(&path).unwrap();
    let mut doc = Document::new("motion", Size::new(64, 64), photostore_color::ColorMode::Rgb, SampleType::U8);
    let first = crate::media_backend::decode_single_frame_at(&path, 0.0, doc.pixel_format()).unwrap();
    let mut layer = Layer::raster("motion", doc.pixel_format());
    let mut video = VideoData::new(vec![first.clone()], 30.0);
    video.source = VideoSource::File { path: path.to_string_lossy().into_owned() };
    video.movie_backing = Some(MovieBacking { frame_count: 90, source_fps: 30.0, trim_in_frame: 0, trim_out_frame: 89 });
    layer.content = LayerContent::Raster(first);
    layer.video = Some(video);
    doc.layers.push(layer);
    let layer_id = doc.layers[0].id;
    let mut timeline = Timeline::new(90, 30.0);
    timeline.set_project_rate(photostore_doc::RationalRate::new(30000, 1001).unwrap()).unwrap();
    crate::audio_cmds::add_linked_movie_audio(&mut timeline, layer_id, path.to_string_lossy().into_owned(), info.duration_seconds, 0.0, 3.0, 0.0, 1.0).unwrap();
    doc.timeline = Some(timeline);
    crate::media_asset_cmds::initialize_sources(&mut doc).unwrap();
    let mut session = Session::new();
    session.add_document(doc, None);
    let initial = frame_hash(&session.active().unwrap().doc);
    let history = session.active().unwrap().history.past_len();
    let generation = session.active().unwrap().content_revision;
    let mut timings = Vec::new();
    {
        let _preview = crate::movie_cache::PreviewScope::enter();
        for target in [15, 16, 30, 60, 88, 89] {
            let begin = Instant::now();
            let mut misses = 0;
            loop {
                let before = session.active().unwrap().doc.timeline.as_ref().unwrap().current;
                let result = session.execute("timeline.presentFrame", json!({"frame":target})).unwrap();
                if result["ready"] == true {
                    break;
                }
                assert_eq!(session.active().unwrap().doc.timeline.as_ref().unwrap().current, before);
                misses += 1;
                assert!(begin.elapsed() < Duration::from_secs(10), "requested frame never became presentable");
                std::thread::sleep(Duration::from_millis(5));
            }
            timings.push((target, begin.elapsed().as_secs_f64() * 1000.0, misses));
            assert_ne!(frame_hash(&session.active().unwrap().doc), initial);
            let shown=&session.active().unwrap().doc;
            let time=crate::source_time_for_layer(shown,shown.layer(layer_id).unwrap(),target).unwrap();
            let surface=crate::media_backend::decode_single_frame_at(&path,time,shown.pixel_format()).unwrap();
            let mut exact=(**shown).clone();exact.layers[0].content=LayerContent::Raster(surface);
            assert_eq!(frame_hash(shown),frame_hash(&exact),"streamed preview must retain the exact held source frame at {target}");
            let budget = crate::media_budget::status();
            assert!(budget.total_used_bytes <= budget.total_limit_bytes);
        }
    }
    let state = session.active().unwrap();
    assert_eq!(state.history.past_len(), history);
    assert_eq!(state.content_revision, generation);
    assert_eq!(state.saved_revision, state.revision);
    let before_refresh=session.active().unwrap().revision;
    assert!(session.refresh_linked_movie_preview());
    assert_eq!(session.active().unwrap().revision,before_refresh,"unchanged cached pixels must not force another canvas refresh");
    let output = std::env::temp_dir().join(format!("photostore-timeline-export-{}.mp4", std::process::id()));
    session.execute("file.export.renderVideo", json!({"path":output,"format":"mp4","start":15,"end":17,"background":"black","overwrite":true})).unwrap();
    let exported = crate::media_backend::probe(&output).unwrap();
    assert_eq!((exported.fps.num, exported.fps.den), (30000, 1001));
    assert_eq!(exported.frame_count, Some(3));
    assert!(exported.has_audio);
    let exported_audio=crate::media_backend::probe_audio(&output).unwrap();
    assert!((exported_audio.duration_seconds-3.0*1001.0/30000.0).abs()<0.002,"audio tail was truncated: {}",exported_audio.duration_seconds);
    assert_eq!(exported.color_transfer.as_deref(), Some("bt709"));
    assert_eq!(exported.color_primaries.as_deref(), Some("bt709"));
    assert_eq!(session.active().unwrap().doc.timeline.as_ref().unwrap().current, 89);
    assert_eq!(session.active().unwrap().history.past_len(), history);
    let old_time = crate::source_time_for_layer(&session.active().unwrap().doc, session.active().unwrap().doc.layer(layer_id).unwrap(), 30).unwrap();
    let old_audio = session.active().unwrap().doc.timeline.as_ref().unwrap().audio_tracks[0].clips[0].clone();
    session.execute("timeline.setProps", json!({"fpsNumerator":24000,"fpsDenominator":1001})).unwrap();
    let new_time = crate::source_time_for_layer(&session.active().unwrap().doc, session.active().unwrap().doc.layer(layer_id).unwrap(), 30).unwrap();
    assert!((old_time - new_time).abs() < 1e-9);
    let new_audio = &session.active().unwrap().doc.timeline.as_ref().unwrap().audio_tracks[0].clips[0];
    assert!((old_audio.duration_seconds * old_audio.speed - new_audio.duration_seconds * new_audio.speed).abs() < 1e-9);
    assert!(session.undo());
    session.execute("timeline.editor.split", json!({"layer":layer_id.0,"frame":17})).unwrap();
    let clips = &session.active().unwrap().doc.timeline.as_ref().unwrap().audio_tracks[0].clips;
    assert_eq!(clips.len(), 2);
    let right = clips.iter().max_by(|a, b| a.start_seconds.total_cmp(&b.start_seconds)).unwrap();
    assert!((right.trim_in_seconds * 48000.0 - (right.trim_in_seconds * 48000.0).round()).abs() < 1e-7);
    assert!(session.undo());
    assert_eq!(session.active().unwrap().doc.timeline.as_ref().unwrap().audio_tracks[0].clips.len(), 1);
    eprintln!("Synthetic 64x64 FFV1/30fps presentation (frame, ms, misses): {timings:?}; output rate {}/{}", exported.fps.num, exported.fps.den);
    let _ = std::fs::remove_file(output);
}

#[test]
fn scalable_playback_smart_transform_bake_roundtrip_retains_editable_source() {
    let mut session = Session::new();
    session.execute("file.new", json!({"width":64,"height":48,"depth":8})).unwrap();
    let mut inner = Document::new("source", Size::new(16, 12), photostore_color::ColorMode::Rgb, SampleType::U8);
    inner.layers.push(Layer::raster("source", inner.pixel_format()));
    let bytes = photostore_format::save_to_bytes(&inner, &Default::default()).unwrap();
    let source = photostore_doc::SmartSource::Embedded { file_name: "source.pcraft".into(), bytes: Arc::new(bytes) };
    let layer = Layer::new("Smart", LayerContent::Smart(photostore_doc::SmartObject::new(source, photostore_geom::Affine::translate(18.0, 10.0), None)));
    let id = layer.id;
    session
        .edit("setup", |doc, active| {
            doc.layers.push(layer);
            doc.timeline = Some(Timeline::new(4, 30.0));
            *active = Some(id);
            Ok(())
        })
        .unwrap();
    session.execute("timeline.setTransformAnimation", json!({"layer":id.0,"enabled":true})).unwrap();
    session.execute("timeline.setTransformKeyframe", json!({"layer":id.0,"frame":0,"scaleX":1.0,"scaleY":1.0,"rotation":0.0})).unwrap();
    session.execute("timeline.setTransformKeyframe", json!({"layer":id.0,"frame":3,"scaleX":2.0,"scaleY":1.5,"rotation":45.0})).unwrap();
    let matrix = |doc: &Document| match &doc.layer(id).unwrap().content {
        LayerContent::Smart(s) => s.transform.m,
        _ => panic!("source was flattened"),
    };
    let mut expected = Vec::new();
    for frame in 0..4 {
        session.execute("timeline.setFrame", json!({"frame":frame})).unwrap();
        expected.push(matrix(&session.active().unwrap().doc));
    }
    session.execute("timeline.setMode", json!({"mode":"frameAnimation"})).unwrap();
    for frame in 0..4 {
        session.execute("timeline.setFrame", json!({"frame":frame})).unwrap();
        assert_eq!(matrix(&session.active().unwrap().doc), expected[frame]);
    }
    let bytes = photostore_format::save_to_bytes(&session.active().unwrap().doc, &Default::default()).unwrap();
    let back = photostore_format::load_from_bytes(&bytes).unwrap();
    assert!(matches!(back.layer(id).unwrap().content, LayerContent::Smart(_)));
    assert_eq!(back.timeline.as_ref().unwrap().frame_states[3].iter().find(|s| s.layer == id).unwrap().smart_transform, Some(expected[3]));
    session.execute("timeline.setMode", json!({"mode":"video"})).unwrap();
    for frame in 0..4 {
        session.execute("timeline.setFrame", json!({"frame":frame})).unwrap();
        assert_eq!(matrix(&session.active().unwrap().doc), expected[frame]);
    }
}

#[test]
fn scalable_playback_proxy_never_replaces_original_pixels() {
    let Some(path) = fixture() else { return };
    let original_bytes = std::fs::read(&path).unwrap();
    let mut session = Session::new();
    let mut doc = Document::new("proxy boundary", Size::new(64, 64), photostore_color::ColorMode::Rgb, SampleType::U8);
    let info = crate::media_backend::probe(&path).unwrap();
    let first = crate::media_backend::decode_single_frame_at(&path, 0.0, doc.pixel_format()).unwrap();
    let mut layer = Layer::raster("movie", doc.pixel_format());
    let id = layer.id;
    let mut video = VideoData::new(vec![first.clone()], 30.0);
    video.source = VideoSource::File { path: path.to_string_lossy().into_owned() };
    video.movie_backing = Some(MovieBacking { frame_count: 90, source_fps: 30.0, trim_in_frame: 0, trim_out_frame: 89 });
    layer.content = LayerContent::Raster(first);
    layer.video = Some(video);
    doc.layers.push(layer);
    doc.timeline = Some(Timeline::new(90, 30.0));
    crate::media_asset_cmds::initialize_sources(&mut doc).unwrap();
    session.add_document(doc, None);
    session.active_mut().unwrap().active_layer = Some(id);
    session.execute("media.proxy.configure", json!({"enabled":true,"quality":"quarter","limitBytes":1048576})).unwrap();
    let proxy = crate::media_proxy::create(path.to_str().unwrap(), crate::media_proxy::ProxyQuality::Quarter, || false, |_, _| {}).unwrap();
    let proxy_info = crate::media_backend::probe(std::path::Path::new(&proxy.proxy_path)).unwrap();
    assert_eq!((proxy_info.width, proxy_info.height), (16, 16));
    assert!(crate::media_proxy::status().unwrap().used_bytes <= 1048576);
    let expected_surface = crate::media_backend::decode_single_frame_at(&path, 20.0 / 30.0, session.active().unwrap().doc.pixel_format()).unwrap();
    let mut expected = (*session.active().unwrap().doc).clone();
    expected.layers[0].content = LayerContent::Raster(expected_surface);
    let expected_hash = frame_hash(&expected);
    {
        let _preview = crate::movie_cache::PreviewScope::enter();
        let begin = Instant::now();
        loop {
            if session.execute("timeline.presentFrame", json!({"frame":20})).unwrap()["ready"] == true {
                break;
            }
            assert!(begin.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_ne!(frame_hash(&session.active().unwrap().doc), expected_hash, "fixture should distinguish a quarter proxy from originals");
        let (png, _) = crate::file_cmds::encode(&session.active().unwrap().doc, "original.png", None).unwrap();
        let image = crate::file_cmds::import("original.png", &png).unwrap();
        assert_eq!(frame_hash(&image), expected_hash, "image output must use original pixels");
        session.execute("layer.rasterize.video", json!({})).unwrap();
        assert_eq!(frame_hash(&session.active().unwrap().doc), expected_hash, "rasterizing must use original pixels");
        assert!(session.active().unwrap().doc.layer(id).unwrap().video.is_none());
        session.execute("edit.undo", json!({})).unwrap();
        assert!(session.active().unwrap().doc.layer(id).unwrap().video.is_some());
    }
    session.execute("media.proxy.clear", json!({})).unwrap();
    session.execute("media.proxy.configure", json!({"enabled":false,"quality":"original"})).unwrap();
    assert_eq!(crate::media_proxy::status().unwrap().used_bytes, 0);
    assert_eq!(std::fs::read(&path).unwrap(), original_bytes, "cache operations must never touch source bytes");
    assert!(info.duration_seconds > 0.0);
}

#[test]
fn scalable_playback_real_size_presentation_throughput() {
    let Some(root)=std::env::var_os("PHOTOSTORE_SCALABLE_FIXTURES") else {eprintln!("Skipping real-size timing: set PHOTOSTORE_SCALABLE_FIXTURES to owned fixture directory");return;};
    for label in ["1080p","4k"] {
        let path=std::path::Path::new(&root).join(format!("photostore-playback-{label}-30fps.mp4"));
        assert!(path.is_file(),"missing explicit benchmark fixture");
        crate::movie_cache::clear();
        let result=crate::media_performance::measure(path.to_str().unwrap(),90,8usize<<30).unwrap();
        eprintln!("SCALABLE_PLAYBACK_MEASUREMENT {}",serde_json::to_string(&result).unwrap());
        assert!(result["distinctSampledPixelHashes"].as_u64().unwrap()>10,"pixels must change, not just clock values");
        assert_eq!(result["historyUnchanged"],true);assert_eq!(result["clean"],true);
        assert!(result["persistentDecoderLaunches"].as_u64().unwrap()<=5,"forward playback must reuse a decoder");
        assert!(result["decodeProcessLaunches"].as_u64().unwrap()<=5,"exact-frame fallbacks must not hide per-frame process launches");
        assert!(result["maxRetainedDerivedBytes"].as_u64().unwrap()<=result["retainedBudgetBytes"].as_u64().unwrap());
        if label=="1080p" {
            assert!(result["warmFramesPerSecond"].as_f64().unwrap()>=27.0,"1080p core preparation is below the planned real-time goal");
            assert!(result["warmP95Ms"].as_f64().unwrap()<=33.4,"1080p warm presentation misses the 30fps frame budget");
        }
    }
}

#[test]
fn scalable_playback_real_size_presentation_throughput_under_cache_pressure() {
    let Some(root)=std::env::var_os("PHOTOSTORE_SCALABLE_FIXTURES") else{return};
    let path=std::path::Path::new(&root).join("photostore-playback-4k-30fps.mp4");
    crate::movie_cache::clear();
    let result=crate::media_performance::measure(path.to_str().unwrap(),12,256usize<<20).unwrap();
    assert!(result["distinctSampledPixelHashes"].as_u64().unwrap()>3);
    assert!(result["decodeProcessLaunches"].as_u64().unwrap()<=3,"a one-frame cache must backpressure instead of restarting for every frame");
    assert!(result["maxRetainedDerivedBytes"].as_u64().unwrap()<=result["retainedBudgetBytes"].as_u64().unwrap());
    eprintln!("SCALABLE_PLAYBACK_PRESSURE {}",serde_json::to_string(&result).unwrap());
    crate::media_budget::configure(8usize<<30);
}

#[test]
fn layer_workflow_boundary_deferred_video_roundtrip_activation_and_undo() {
    let Some(path) = fixture() else { return };
    let mut session = Session::new();
    session.execute("file.new", json!({"width":64,"height":64})).unwrap();
    let result = session.execute("layer.videoLayers.newVideoLayerFromFile", json!({"path":path,"createTimeline":false})).unwrap();
    let layer = photostore_doc::LayerId(result["layer"].as_u64().unwrap());
    let doc = &session.active().unwrap().doc;
    assert!(doc.timeline.is_none(), "a document drop must not implicitly create a timeline");
    assert!(doc.layer(layer).unwrap().video.as_ref().unwrap().playback.awaiting_video_timeline);
    assert!(doc.media_assets.iter().any(|asset| asset.metadata.as_ref().is_some_and(|metadata| metadata.audio.is_some())));
    let bytes = photostore_format::save_to_bytes(doc, &Default::default()).unwrap();
    let loaded = photostore_format::load_from_bytes(&bytes).unwrap();
    let mut reopened = Session::new();
    reopened.add_document(loaded, None);
    reopened.execute("timeline.createFrameAnimation", json!({"duration":30})).unwrap();
    reopened.execute("timeline.setFrame", json!({"frame":10})).unwrap();
    let doc = &reopened.active().unwrap().doc;
    assert_eq!(crate::source_time_for_layer(doc, doc.layer(layer).unwrap(), 10).unwrap(), crate::source_time_for_layer(doc, doc.layer(layer).unwrap(), 0).unwrap(), "Frame Animation must keep a deferred movie static");
    let before = (**doc).clone();
    reopened.execute("timeline.create", json!({})).unwrap();
    let doc = &reopened.active().unwrap().doc;
    let timeline = doc.timeline.as_ref().unwrap();
    assert_eq!(timeline.duration, 90);
    assert_eq!(timeline.work_end, 90, "the full imported clip must be playable, not only the original default 30 frames");
    assert!(timeline.layer_ranges.iter().any(|range| range.layer==layer && range.end==90));
    assert!(!doc.layer(layer).unwrap().video.as_ref().unwrap().playback.awaiting_video_timeline);
    let audio: Vec<_> = timeline.audio_tracks.iter().flat_map(|track| &track.clips).filter(|clip| clip.linked_video==Some(layer)).collect();
    assert_eq!(audio.len(), 1);
    assert!(audio[0].asset_id.is_some());
    assert!(reopened.undo());
    assert_eq!(*reopened.active().unwrap().doc, before, "activation and linked audio must undo as one edit");
    reopened.edit("Oversized source boundary fixture", |doc, _| {
        let video = doc.layer_mut(layer).unwrap().video.as_mut().unwrap();
        let backing = video.movie_backing.as_mut().unwrap();
        backing.frame_count = 1_000_000;
        backing.trim_out_frame = 999_999;
        Ok(())
    }).unwrap();
    let before = reopened.active().unwrap().doc.clone();
    let history = reopened.active().unwrap().history.past_len();
    assert!(reopened.execute("timeline.create", json!({"fps":1000})).is_err());
    assert_eq!(reopened.active().unwrap().doc, before, "failed activation must not replace the old timeline or partial audio");
    assert_eq!(reopened.active().unwrap().history.past_len(), history);
}

#[test]
fn layer_workflow_boundary_copy_preserves_source_and_catalog_identity() {
    let Some(path) = fixture() else { return };
    let mut session = Session::new();
    session.execute("file.new", json!({"width":64,"height":64})).unwrap();
    let result = session.execute("layer.videoLayers.newVideoLayerFromFile", json!({"path":path,"createTimeline":false})).unwrap();
    let layer = photostore_doc::LayerId(result["layer"].as_u64().unwrap());
    let source = session.active().unwrap().doc.clone();
    let source_asset = source.layer(layer).unwrap().video.as_ref().unwrap().asset_id.unwrap();
    session.execute("file.new", json!({"width":64,"height":64})).unwrap();
    let destination_background = session.active().unwrap().doc.layers[0].id;
    let result = session.execute("layer.copyToDocument", json!({"source":0,"document":1,"layers":[layer.0],"above":destination_background.0,"name":"Copied movie"})).unwrap();
    let copy = photostore_doc::LayerId(result["layers"][0].as_u64().unwrap());
    assert_eq!(session.documents()[0].doc, source, "a copy must preserve the source document");
    let target = &session.active().unwrap().doc;
    let asset = target.layer(copy).unwrap().video.as_ref().unwrap().asset_id.unwrap();
    assert_ne!(asset, source_asset, "catalog identities belong to their destination document");
    let original_asset = source.media_assets.iter().find(|asset| asset.id==source_asset).unwrap();
    let copied_asset = target.media_assets.iter().find(|entry| entry.id==asset).unwrap();
    assert_eq!(copied_asset.path, original_asset.path);
    assert_eq!(copied_asset.metadata, original_asset.metadata);
    assert!(target.timeline.is_none());
    assert!(session.undo());
    assert!(session.active().unwrap().doc.media_assets.is_empty());
    session.set_active(0);
    let above = session.active().unwrap().doc.layers[0].id;
    let result = session.execute("layer.copyToDocument", json!({"source":0,"document":0,"allowSame":true,"layers":[layer.0],"above":above.0,"name":"Placed copy"})).unwrap();
    let copy = photostore_doc::LayerId(result["layers"][0].as_u64().unwrap());
    assert_eq!(session.active().unwrap().doc.layers[1].id, copy);
    assert_eq!(session.active().unwrap().doc.layer(copy).unwrap().video.as_ref().unwrap().asset_id, Some(source_asset));
    let before = session.active().unwrap().doc.clone();
    assert!(session.execute("layer.copyToDocument", json!({"document":0,"allowSame":true,"layers":[layer.0],"above":999_999})).is_err());
    assert_eq!(session.active().unwrap().doc, before);
}
