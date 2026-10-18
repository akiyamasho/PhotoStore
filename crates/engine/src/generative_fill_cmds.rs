//! Selection-bounded, provider-backed generative fill.
use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};
use photostore_cms::Builtin;
use photostore_color::ColorMode;
use photostore_doc::{Layer, LayerContent};
use photostore_geom::Rect;
use photostore_raster::{Surface, from_rgba_into};
use serde_json::{Value, json};
use std::{
    fs,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const CMD: &str = "edit.generativeFill";
const MAX_PIXELS: usize = 25_000_000;
fn enabled(s: &Session) -> std::result::Result<(), String> {
    let d = s.active().ok_or("open a document first")?;
    let sel = d.doc.selection.as_ref().ok_or("make a non-empty selection first")?;
    if sel.content_bounds().intersect(&d.doc.bounds()).is_empty() {
        return Err("make a non-empty selection first".into());
    }
    Ok(())
}
fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let prompt = p.get("prompt").and_then(Value::as_str).ok_or_else(|| bad("prompt must be a string"))?.trim().to_owned();
    if prompt.is_empty() || prompt.len() > 32_768 {
        return Err(bad("prompt must contain 1–32768 bytes"));
    }
    let prefs = s.prefs().generative_fill.clone();
    let provider = match p.get("provider") {
        Some(v) => v.as_str().ok_or_else(|| bad("provider must be a string"))?.to_owned(),
        None => prefs.provider,
    };
    if provider != "codex" && provider != "custom" {
        return Err(bad("provider must be `codex` or `custom`"));
    }
    let executable = match p.get("executable") {
        Some(v) => v.as_str().ok_or_else(|| bad("executable must be a string"))?.to_owned(),
        None => prefs.executable,
    };
    if executable.trim().is_empty() || executable.contains('\0') {
        return Err(bad("executable must be a nonempty executable name or path"));
    }
    let timeout = match p.get("timeoutSeconds") {
        Some(v) => v.as_u64().ok_or_else(|| bad("timeoutSeconds must be an integer from 1 to 3600"))?,
        None => prefs.timeout_seconds,
    };
    if !(1..=3600).contains(&timeout) {
        return Err(bad("timeoutSeconds must be from 1 to 3600"));
    }
    let args: Vec<String> = match p.get("args") {
        Some(v) => serde_json::from_value(v.clone()).map_err(|_| bad("args must be an array of strings"))?,
        None => prefs.args,
    };
    if args.len() > 256 || args.iter().any(|a| a.len() > 131_072 || a.contains('\0')) {
        return Err(bad("provider arguments exceed limits (256 arguments, 128 KiB each, no NUL bytes)"));
    }
    let seamless = match p.get("seamless") {
        Some(v) => v.as_bool().ok_or_else(|| bad("seamless must be a boolean"))?,
        None => prefs.seamless,
    };
    let result_provider = provider.clone();
    crate::jobs::edit_job(
        s,
        "Generative Fill",
        move |doc, active, ctx| {
            let sel = doc.selection.clone().ok_or_else(|| EngineError::Other("make a non-empty selection first".into()))?;
            if doc.mode != ColorMode::Rgb {
                return Err(EngineError::Other("Generative Fill currently supports RGB documents only".into()));
            }
            if !crate::color_cmds::composite_profile(doc).same_colors(Builtin::Srgb.profile()) {
                return Err(EngineError::Other("Generative Fill currently supports sRGB RGB documents only; convert the document to sRGB first".into()));
            }
            let canvas = doc.bounds();
            let canvas_pixels = (canvas.width() as usize)
                .checked_mul(canvas.height() as usize)
                .filter(|n| *n <= MAX_PIXELS)
                .ok_or_else(|| EngineError::Other("canvas exceeds the 25 million pixel generative fill limit".into()))?;
            let rect = sel.content_bounds().intersect(&canvas);
            if rect.is_empty() {
                return Err(EngineError::Other("make a non-empty selection first".into()));
            }
            let dir = std::env::temp_dir().join(format!(
                "photostore-generative-fill-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&dir).map_err(ioerr)?;
            let _workspace = Workspace(dir.clone());
            let result = (|| {
                ctx.check()?;
                let context_path = dir.join("context.png");
                let mask_path = dir.join("mask.png");
                let crop_path = dir.join("crop.png");
                let output_path = dir.join("output.png");
                let request_path = dir.join("request.json");
                let rendered = photostore_compose::render(doc, canvas).to_rgba8();
                let context_img = photostore_codecs::Image::from_u8(canvas.width(), canvas.height(), photostore_codecs::ChannelLayout::Rgba, rendered.pixels)
                    .map_err(|e| EngineError::Other(e.to_string()))?;
                fs::write(
                    &context_path,
                    photostore_codecs::encode(&context_img, photostore_codecs::Format::Png, &Default::default())
                        .map_err(|e| EngineError::Other(e.to_string()))?,
                )
                .map_err(ioerr)?;
                let (w, h) = (rect.width() as u32, rect.height() as u32);
                let pixels = (w as usize)
                    .checked_mul(h as usize)
                    .filter(|n| *n <= MAX_PIXELS)
                    .ok_or_else(|| EngineError::Other("selection exceeds the 25 million pixel generative fill limit".into()))?;
                let mut mask = Vec::with_capacity(canvas_pixels);
                for y in canvas.y0..canvas.y1 {
                    for x in canvas.x0..canvas.x1 {
                        mask.push(if x >= rect.x0 && x < rect.x1 && y >= rect.y0 && y < rect.y1 {
                            (sel.sample_channel(x, y, 0).clamp(0., 1.) * 255.).round() as u8
                        } else {
                            0
                        });
                    }
                }
                let mask_img = photostore_codecs::Image::from_u8(canvas.width(), canvas.height(), photostore_codecs::ChannelLayout::Gray, mask)
                    .map_err(|e| EngineError::Other(e.to_string()))?;
                fs::write(
                    &mask_path,
                    photostore_codecs::encode(&mask_img, photostore_codecs::Format::Png, &Default::default()).map_err(|e| EngineError::Other(e.to_string()))?,
                )
                .map_err(ioerr)?;
                let context = &context_img;
                let mut crop = Vec::with_capacity(pixels.saturating_mul(4));
                for y in rect.y0..rect.y1 {
                    for x in rect.x0..rect.x1 {
                        let i = ((y - canvas.y0) as usize * canvas.width() as usize + (x - canvas.x0) as usize) * 4;
                        for c in 0..4 {
                            crop.push((context.sample_normalized(i + c).clamp(0., 1.) * 255.).round() as u8);
                        }
                    }
                }
                let crop_img =
                    photostore_codecs::Image::from_u8(w, h, photostore_codecs::ChannelLayout::Rgba, crop).map_err(|e| EngineError::Other(e.to_string()))?;
                fs::write(
                    &crop_path,
                    photostore_codecs::encode(&crop_img, photostore_codecs::Format::Png, &Default::default()).map_err(|e| EngineError::Other(e.to_string()))?,
                )
                .map_err(ioerr)?;
                let request = json!({"schema":"photostore.generative-fill.v1","canvas":{"width":canvas.width(),"height":canvas.height()},"selectionRect":{"x":rect.x0,"y":rect.y0,"width":w,"height":h},"prompt":prompt,"colorContract":"sRGB RGBA PNG; output must exactly match selectionRect or full canvas dimensions","outputPath":output_path,"contextPath":context_path,"maskPath":mask_path,"cropPath":crop_path});
                fs::write(&request_path, serde_json::to_vec_pretty(&request).map_err(|e| EngineError::Other(e.to_string()))?).map_err(ioerr)?;
                let task = format!(
                    "$imagegen Perform a real image generation/edit for PhotoStore generative fill. Do not draw, synthesize, or fake image pixels with code. If image generation is unavailable, fail clearly. Read request.json. Preserve context outside the white mask selection. Write output.png matching exactly the selected crop ({}x{}) or full canvas ({}x{}). Prompt: {}",
                    w,
                    h,
                    canvas.width(),
                    canvas.height(),
                    prompt
                );
                let mut cmd = Command::new(resolve_executable(&provider, &executable));
                if provider == "codex" {
                    cmd.args(["exec", "--ephemeral", "--sandbox", "workspace-write", "--skip-git-repo-check", "-c", r#"approval_policy="never""#]);
                    cmd.arg(format!("--image={}", context_path.display()))
                        .arg(format!("--image={}", mask_path.display()))
                        .arg(format!("--image={}", crop_path.display()))
                        .arg(task);
                } else {
                    for a in &args {
                        cmd.arg(
                            a.replace("{request}", &request_path.to_string_lossy())
                                .replace("{output}", &output_path.to_string_lossy())
                                .replace("{context}", &context_path.to_string_lossy())
                                .replace("{mask}", &mask_path.to_string_lossy())
                                .replace("{crop}", &crop_path.to_string_lossy())
                                .replace("{prompt}", &prompt),
                        );
                    }
                }
                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt;
                    cmd.process_group(0);
                }
                let mut child = cmd.current_dir(&dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| {
                    EngineError::Other(format!("cannot start {provider} provider `{executable}`: {e}; check the executable and provider login"))
                })?;
                let start = Instant::now();
                loop {
                    if ctx.cancelled() {
                        terminate(&mut child);
                        return Err(EngineError::Cancelled);
                    }
                    let status = match child.try_wait() {
                        Ok(status) => status,
                        Err(e) => {
                            terminate(&mut child);
                            return Err(ioerr(e));
                        }
                    };
                    if let Some(status) = status {
                        if !status.success() {
                            return Err(EngineError::Other(format!(
                                "{provider} provider failed ({status}); check provider login, image-tool availability, and arguments"
                            )));
                        }
                        break;
                    }
                    if start.elapsed() > Duration::from_secs(timeout) {
                        terminate(&mut child);
                        return Err(EngineError::Other(format!("generative fill exceeded {timeout}s timeout")));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                ctx.check()?;
                let meta = fs::metadata(&output_path).map_err(|e| EngineError::Other(format!("provider did not write output.png: {e}")))?;
                if meta.len() > 128 * 1024 * 1024 {
                    return Err(EngineError::Other("provider PNG exceeds the 128 MiB output limit".into()));
                }
                let mut encoded = Vec::new();
                fs::File::open(&output_path).map_err(ioerr)?.take((128 << 20) + 1).read_to_end(&mut encoded).map_err(ioerr)?;
                if encoded.len() > 128 << 20 {
                    return Err(EngineError::Other("provider PNG exceeds the 128 MiB output limit".into()));
                }
                if encoded.len() < 24 || !encoded.starts_with(b"\x89PNG\r\n\x1a\n") {
                    return Err(EngineError::Other("provider output must be a PNG file".into()));
                }
                let png_w = u32::from_be_bytes([encoded[16], encoded[17], encoded[18], encoded[19]]);
                let png_h = u32::from_be_bytes([encoded[20], encoded[21], encoded[22], encoded[23]]);
                let png_pixels = (png_w as usize)
                    .checked_mul(png_h as usize)
                    .filter(|n| *n <= MAX_PIXELS)
                    .ok_or_else(|| EngineError::Other("provider PNG exceeds the 25 million pixel limit".into()))?;
                let _ = png_pixels;
                if (png_w, png_h) != (w, h) && (png_w, png_h) != (canvas.width(), canvas.height()) {
                    return Err(EngineError::Other(format!(
                        "provider output must be {w}x{h} or {}x{}; received {png_w}x{png_h}; rescaling is refused",
                        canvas.width(),
                        canvas.height()
                    )));
                }
                let max_width = canvas.width().max(w);
                let max_height = canvas.height().max(h);
                let decode = photostore_codecs::DecodeOptions {
                    limits: photostore_codecs::Limits { max_width, max_height, max_pixels: MAX_PIXELS as u64, max_alloc: 128 << 20 },
                    keep_orientation: true,
                };
                let generated = photostore_codecs::decode_with(&encoded, &decode)
                    .map_err(|e| EngineError::Other(format!("provider output is not a valid bounded PNG: {e}")))?;
                let full = generated.dimensions() == (canvas.width(), canvas.height());
                if !full && generated.dimensions() != (w, h) {
                    return Err(EngineError::Other(format!(
                        "provider output must be {w}x{h} or {}x{}; received {}x{}; rescaling is refused",
                        canvas.width(),
                        canvas.height(),
                        generated.width(),
                        generated.height()
                    )));
                }
                if generated.sample_type() != photostore_codecs::SampleType::U8 {
                    return Err(EngineError::Other("provider PNG must use 8-bit samples".into()));
                }
                if generated.layout() != photostore_codecs::ChannelLayout::Rgb && generated.layout() != photostore_codecs::ChannelLayout::Rgba {
                    return Err(EngineError::Other("provider output must use RGB or RGBA color".into()));
                }
                if let Some(profile) = generated.icc.as_deref() {
                    let matches = photostore_cms::Profile::parse(profile).ok().is_some_and(|p| p.same_colors(Builtin::Srgb.profile()));
                    if !matches {
                        return Err(EngineError::Other("provider PNG must use sRGB color".into()));
                    }
                }
                let mut patch = Vec::with_capacity(pixels);
                for y in 0..h {
                    ctx.check()?;
                    for x in 0..w {
                        let (gx, gy) = if full { (x + (rect.x0 - canvas.x0) as u32, y + (rect.y0 - canvas.y0) as u32) } else { (x, y) };
                        let n = generated.layout().channels();
                        let base = (gy as usize * generated.width() as usize + gx as usize) * n;
                        let rgba = [
                            generated.data()[base] as f32 / 255.,
                            generated.data()[base + 1] as f32 / 255.,
                            generated.data()[base + 2] as f32 / 255.,
                            if n == 4 { generated.data()[base + 3] as f32 / 255. } else { 1. },
                        ];
                        patch.push(rgba);
                    }
                }
                if seamless {
                    match_boundary(&mut patch, context, canvas, rect, &sel, ctx)?;
                }
                ctx.check()?;
                let fmt = doc.pixel_format();
                let mut data = Vec::with_capacity(pixels * fmt.channels());
                let mut native = [0.0f32; 5];
                for (i, px) in patch.iter().enumerate() {
                    let coverage = sel.sample_channel(rect.x0 + (i % w as usize) as i32, rect.y0 + (i / w as usize) as i32, 0).clamp(0., 1.);
                    let n = from_rgba_into(&fmt, [px[0], px[1], px[2], px[3] * coverage], &mut native);
                    data.extend_from_slice(&native[..n]);
                }
                let mut surface = Surface::new(fmt);
                surface.write_region(Rect::new(rect.x0, rect.y0, rect.x1, rect.y1), &data);
                doc.layers.push(Layer::new("Generative Fill", LayerContent::Raster(surface)));
                *active = doc.layers.last().map(|l| l.id);
                Ok([rect.x0, rect.y0, w as i32, h as i32])
            })();
            result
        },
        move |bounds| json!({"bounds":bounds,"provider":result_provider}),
    )
}
#[cfg(unix)]
fn terminate(child: &mut std::process::Child) {
    let group = format!("-{}", child.id());
    let _ = Command::new("/bin/kill").args(["-TERM", "--", &group]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    let _ = child.kill();
    let _ = child.wait();
    let _ = Command::new("/bin/kill").args(["-KILL", "--", &group]).stdout(Stdio::null()).stderr(Stdio::null()).status();
}
#[cfg(not(unix))]
fn terminate(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}
fn ioerr(e: std::io::Error) -> EngineError {
    EngineError::Other(format!("generative fill I/O: {e}"))
}
fn bad(msg: &str) -> EngineError {
    EngineError::BadParams { cmd: CMD.into(), msg: msg.into() }
}
pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Generative Fill…",
        menu: &["Edit"],
        shortcut: None,
        params: r#"{"prompt":string,"provider":"codex"|"custom"?,"executable":string?,"args":[string]?,"timeoutSeconds":u64?,"seamless":bool=true} (requires a non-empty selection; background job)"#,
        enabled,
        run,
        journal: true,
    }]
}

// Own the temporary files even if a codec or provider worker unwinds.
struct Workspace(PathBuf);
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn resolve_executable(provider: &str, executable: &str) -> PathBuf {
    if provider == "codex" && executable == "codex" {
        // Finder launches often have only /usr/bin:/bin in PATH.
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                let candidate = dir.join("codex");
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
        for path in ["/opt/homebrew/bin/codex", "/usr/local/bin/codex"] {
            if std::path::Path::new(path).is_file() {
                return path.into();
            }
        }
    }
    executable.into()
}

/// Local UI readiness check. Does not send images or start an interactive login.
/// Custom adapters are only checked for a runnable executable; their service auth is adapter-owned.
pub fn check_setup(p: &crate::prefs::GenerativeFillPrefs) -> std::result::Result<(), String> {
    if p.provider != "codex" && p.provider != "custom" {
        return Err("Choose a provider in Settings.".into());
    }
    if p.executable.trim().is_empty() {
        return Err("Choose an executable in Settings.".into());
    }
    let resolved = resolve_executable(&p.provider, &p.executable);
    let runnable = |path: &std::path::Path| {
        let Ok(metadata) = path.metadata() else { return false };
        if !metadata.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                return false;
            }
        }
        true
    };
    let executable = if resolved.components().count() == 1 {
        std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|d| d.join(&resolved)).find(|p| runnable(p)))
    } else {
        runnable(&resolved).then_some(resolved)
    };
    let executable = executable.ok_or_else(|| "Local provider is not installed or its executable cannot be found.".to_owned())?;
    if p.provider == "custom" {
        return Ok(());
    }
    let mut child = Command::new(executable)
        .args(["login", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Codex could not start. Check its executable in Settings.".to_owned())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return if status.success() { Ok(()) } else { Err("Codex is not signed in. Complete local setup in Settings.".into()) },
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Codex setup check could not finish. Open Settings to try again.".into());
            }
        }
    }
}

/// Match generated gradients to the existing image at the editable region's inner boundary.
/// The boundary is on selected pixels, so even an arbitrary mask never changes preserved pixels.
fn match_boundary(
    patch: &mut [[f32; 4]],
    context: &photostore_codecs::Image,
    canvas: Rect,
    rect: Rect,
    selection: &Surface,
    ctx: &crate::jobs::JobCtx,
) -> Result<()> {
    let (w, h) = (rect.width() as usize, rect.height() as usize);
    let mut domain = Vec::with_capacity(patch.len());
    let selected = |x: i32, y: i32| selection.sample_channel(x, y, 0) > 0.;
    for (i, px) in patch.iter().enumerate() {
        let (x, y) = (rect.x0 + (i % w) as i32, rect.y0 + (i / w) as i32);
        let boundary = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
            .iter()
            .any(|&(nx, ny)| nx >= canvas.x0 && nx < canvas.x1 && ny >= canvas.y0 && ny < canvas.y1 && !selected(nx, ny));
        domain.push(selected(x, y) && px[3] > 0. && !boundary);
    }
    for channel in 0..3 {
        ctx.check()?;
        let original = |i: usize| {
            let (x, y) = (rect.x0 - canvas.x0 + (i % w) as i32, rect.y0 - canvas.y0 + (i / w) as i32);
            let at = (y as usize * canvas.width() as usize + x as usize) * 4;
            if patch[i][3] > 0. && context.sample_normalized(at + 3) > 0. { context.sample_normalized(at + channel) } else { patch[i][channel] }
        };
        let difference: Vec<f32> = patch.iter().enumerate().map(|(i, px)| original(i) - px[channel]).collect();
        let correction = ctx
            .stage(0.8 + channel as f32 * 0.05, 0.85 + channel as f32 * 0.05, "Matching image boundary", |ctl| {
                photostore_algo::poisson::membrane_fill_with(w, h, 1, &difference, &domain, ctl)
            })
            .map_err(|_| EngineError::Cancelled)?;
        for (px, value) in patch.iter_mut().zip(correction) {
            px[channel] = (px[channel] + value).clamp(0., 1.);
        }
    }
    Ok(())
}
