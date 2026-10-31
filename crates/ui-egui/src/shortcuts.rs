//! Keyboard shortcuts: Photoshop defaults from the command registry, plus single-key tools.

use egui::{Key, KeyboardShortcut, Modifiers};
use serde_json::json;

use crate::PhotoStoreApp;
use crate::state::Tool;

/// Parse `Cmd+Shift+N` style strings. `Cmd` maps to ⌘ on macOS and Ctrl elsewhere.
pub fn parse(s: &str) -> Option<KeyboardShortcut> {
    let mut mods = Modifiers::NONE;
    let mut key = None;
    for part in s.split('+') {
        match part {
            "Cmd" => mods |= Modifiers::COMMAND,
            "Shift" => mods |= Modifiers::SHIFT,
            "Alt" => mods |= Modifiers::ALT,
            "Ctrl" => mods |= Modifiers::CTRL,
            "" => key = Some(Key::Plus),
            k => {
                key = Key::from_name(k).or(match k {
                    "=" => Some(Key::Equals),
                    "-" => Some(Key::Minus),
                    "[" => Some(Key::OpenBracket),
                    ";" => Some(Key::Semicolon),
                    "'" => Some(Key::Quote),
                    "]" => Some(Key::CloseBracket),
                    _ => None,
                })
            }
        }
    }
    Some(KeyboardShortcut::new(mods, key?))
}

/// Human-readable form for menus, tooltips and inline hints (`Cmd+Shift+N` → `⌘⇧N` on
/// macOS, `Ctrl+Shift+N` elsewhere). Works for lone keys too: `pretty("Alt")` is `⌥`/`Alt`,
/// `pretty("Enter")` is `↩`/`Enter`.
pub fn pretty(s: &str) -> String {
    let mac = cfg!(target_os = "macos");
    s.split('+')
        .map(|p| match (p, mac) {
            ("Cmd", true) => "⌘".to_string(),
            ("Cmd", false) => "Ctrl".to_string(),
            ("Shift", true) => "⇧".to_string(),
            ("Alt", true) => "⌥".to_string(),
            ("Enter", true) => "↩".to_string(),
            ("Delete", true) => "⌫".to_string(),
            (other, _) => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(if mac { "" } else { "+" })
}

/// The command's effective shortcut in the platform's notation (`Ctrl+N`), or `None` when
/// the command has no binding (the user removed it in Edit › Keyboard Shortcuts).
pub fn shortcut_label(app: &PhotoStoreApp, command: &str) -> Option<String> {
    effective_shortcut(app, command, default_shortcut(command).as_deref()).map(|s| pretty(&s))
}

/// `label` (English, shown in the UI language) followed by the command's effective shortcut
/// (`New document…     Ctrl+N`); bare `label` when the binding was removed.
pub fn command_label(app: &PhotoStoreApp, label: &str, command: &str) -> String {
    let label = tl!(label);
    shortcut_label(app, command).map_or_else(|| label.to_string(), |sc| format!("{label}     {sc}"))
}

/// `label` (English, shown in the UI language) with the command's effective shortcut in
/// parentheses (`Rulers  (Ctrl+R)`); bare `label` when the binding was removed.
pub fn tip_label(app: &PhotoStoreApp, label: &str, command: &str) -> String {
    let label = tl!(label);
    shortcut_label(app, command).map_or_else(|| label.to_string(), |sc| format!("{label}  ({sc})"))
}

/// The shortcut in effect for a command: the user's override, else `default`.
pub fn effective_shortcut(app: &PhotoStoreApp, id: &str, default: Option<&str>) -> Option<String> {
    app.session.prefs().shortcut(id, default).map(str::to_string)
}

/// All bindings in effect for a command, preserving the primary key first.
pub fn effective_shortcuts(app: &PhotoStoreApp, id: &str, default: Option<&str>) -> Vec<String> {
    app.session.prefs().shortcuts_for(id, default)
}

/// Default (unmodified) shortcut of a command id.
pub fn default_shortcut(id: &str) -> Option<String> {
    if let Some((_, _, binding)) = TIMELINE_SHORTCUTS.iter().find(|(command, _, _)| *command == id) {
        return (*binding).map(str::to_string);
    }
    if photostore_engine::prefs::is_tool_shortcut_id(id) {
        return photostore_engine::prefs::tool_default_shortcut(id).map(str::to_string);
    }
    crate::menus::UI_COMMANDS
        .iter()
        .find(|c| c.0 == id)
        .and_then(|c| c.3)
        .or_else(|| photostore_engine::commands::find(id).and_then(|c| c.shortcut))
        .or_else(|| crate::menu_catalog::CATALOG.iter().find(|c| c.3 == id).and_then(|c| c.2))
        .or_else(|| photostore_engine::prefs::TEMPORARY_TOOLS.iter().find(|t| t.0 == id).map(|t| t.2))
        .map(str::to_string)
}

/// Timeline-only keys stay out of the global Photoshop dispatcher. They are read here only while
/// the timeline editor owns keyboard focus (after a direct click in its viewport).
pub const TIMELINE_SHORTCUTS: &[(&str, &str, Option<&str>)] = &[
    ("timeline.editor.selectMode", "Select mode", Some("A")),
    ("timeline.editor.razorMode", "Razor mode", Some("B")),
    ("timeline.editor.splitSelected", "Split selected clip", Some("Cmd+B")),
    ("timeline.editor.splitAll", "Split all eligible clips", Some("Cmd+Shift+B")),
    ("timeline.editor.delete", "Delete selected clips", Some("Delete")),
    ("timeline.editor.copy", "Copy selected clips", Some("Cmd+C")),
    ("timeline.editor.copyAttributes", "Copy selected clip attributes", Some("Cmd+Shift+C")),
    ("timeline.editor.pasteAttributes", "Paste clip attributes", Some("Cmd+Shift+V")),
    ("timeline.editor.cut", "Cut selected clips", Some("Cmd+X")),
    ("timeline.editor.paste", "Paste clips", Some("Cmd+V")),
    ("timeline.editor.undo", "Undo timeline edit", Some("Cmd+Z")),
    ("timeline.editor.redo", "Redo timeline edit", Some("Cmd+Shift+Z")),
    ("timeline.editor.playPause", "Play / Pause", Some("Space")),
    ("timeline.editor.shuttleReverse", "Shuttle reverse", Some("J")),
    ("timeline.editor.shuttleStop", "Shuttle stop", Some("K")),
    ("timeline.editor.shuttleForward", "Shuttle forward", Some("L")),
    ("timeline.editor.shuttleReverseAlias", "Shuttle reverse alias", Some("Cmd+P")),
    ("timeline.editor.framePrevious", "Previous frame", Some("Left")),
    ("timeline.editor.frameNext", "Next frame", Some("Right")),
    ("timeline.editor.framePreviousMany", "Previous 10 frames", Some("Shift+Left")),
    ("timeline.editor.frameNextMany", "Next 10 frames", Some("Shift+Right")),
    ("timeline.editor.firstFrame", "Go to first frame", Some("Home")),
    ("timeline.editor.lastFrame", "Go to last frame", Some("End")),
    ("timeline.editor.previousEdit", "Previous edit point", Some("Up")),
    ("timeline.editor.nextEdit", "Next edit point", Some("Down")),
    ("timeline.editor.magnet", "Main track magnet", Some("P")),
    ("timeline.editor.snapping", "Snapping", Some("N")),
    ("timeline.editor.linkedSelection", "Linked selection", Some("Shift+Backtick")),
    ("timeline.editor.previewSkimming", "Preview skimming", Some("S")),
    ("timeline.editor.enableClip", "Enable / Disable clip", Some("V")),
    ("timeline.editor.markerAdd", "Add marker", Some("M")),
    ("timeline.editor.markerNext", "Next marker", Some("Shift+M")),
    ("timeline.editor.markerPrevious", "Previous marker", Some("Alt+Shift+M")),
    ("timeline.editor.workAreaIn", "Set work area start", Some("I")),
    ("timeline.editor.workAreaOut", "Set work area end", Some("O")),
    ("timeline.editor.zoomIn", "Zoom in", Some("Cmd++")),
    ("timeline.editor.zoomOut", "Zoom out", Some("Cmd+-")),
    ("timeline.editor.zoomFit", "Fit timeline", Some("Shift+Z")),
    ("timeline.editor.keyPane", "Show selected clip keyframes", Some("Alt+K")),
    ("timeline.editor.materialBin", "Show / Hide Media bin", Some("Tab")),
    ("timeline.editor.stepLeftAlias", "Previous frame alias", Some("Cmd+Shift+W")),
    ("timeline.editor.stepRightAlias", "Next frame alias", Some("Cmd+Shift+E")),
    ("timeline.editor.selectLeftward", "Select leftward clip", Some("[")),
    ("timeline.editor.selectRightward", "Select rightward clip", Some("]")),
    ("timeline.editor.workAreaSelection", "Set work area to selected clip(s)", Some("Shift+X")),
    ("timeline.editor.deselect", "Clear work area and deselect clips", Some("Alt+X")),
    ("timeline.editor.trimLeftToPlayhead", "Delete left side of selected clip", Some("Q")),
    ("timeline.editor.trimRightToPlayhead", "Delete right side of selected clip", Some("W")),
    ("timeline.editor.group", "Group selected layers", Some("Cmd+G")),
    ("timeline.editor.ungroup", "Ungroup selected layers", Some("Cmd+Shift+G")),
    ("timeline.editor.makeCompound", "Convert selection to Smart Object", Some("Alt+G")),
    ("timeline.editor.unpackCompound", "Convert Smart Object to layers", Some("Alt+Shift+G")),
    ("timeline.editor.speedPanel", "Show clip speed controls", Some("Cmd+R")),
    ("timeline.editor.speedCurve", "Show speed curve controls", Some("Shift+B")),
    ("timeline.editor.audioVolumeUp", "Raise selected audio by 0.1 dB", Some("Cmd+.")),
    ("timeline.editor.audioVolumeDown", "Lower selected audio by 0.1 dB", Some("Cmd+,")),
    ("timeline.editor.addKeyframe", "Add enabled property keyframes", Some("Alt+Shift+K")),
    ("timeline.editor.extractAudio", "Extract / Restore selected video audio", Some("Ctrl+Shift+S")),
    ("timeline.editor.importMedia", "Import source media", Some("Cmd+I")),
    ("timeline.editor.exportVideo", "Render video", Some("Cmd+E")),
    ("timeline.editor.backToEdit", "Back to edit workspace", Some("Cmd+Alt+Q")),
    ("timeline.editor.markerAddColor", "Add colored marker", Some("Alt+M")),
    ("timeline.editor.zoomInEquals", "Zoom in alternate", Some("Cmd+=")),
    ("timeline.editor.enterContents", "Edit selected Smart Object contents", Some("Shift+Enter")),
    ("timeline.editor.playerFullscreen", "Toggle player full screen", Some("Cmd+Shift+F")),
    ("timeline.editor.playerFullscreenLegacy", "Toggle player full screen", Some("Cmd+Ctrl+F")),
    ("timeline.editor.playerZoomIn", "Zoom player in", Some("Alt+Shift++")),
    ("timeline.editor.playerZoomOut", "Zoom player out", Some("Alt+Shift+-")),
    ("timeline.editor.playerZoomFit", "Fit player", Some("Alt+Shift+Z")),
    ("timeline.editor.playerRulers", "Show / Hide rulers", Some("Cmd+Alt+R")),
    ("timeline.editor.playerGuides", "Show / Hide guides", Some("Cmd+;")),
    ("timeline.editor.keyEaseIn", "Ease selected keys in", None),
    ("timeline.editor.keyEaseOut", "Ease selected keys out", None),
    ("timeline.editor.keyEaseInOut", "Ease selected keys in and out", None),
    ("timeline.editor.keyLinear", "Set selected keys to linear", None),
    ("timeline.editor.keyHold", "Hold selected keys", None),
    ("timeline.editor.previousKey", "Previous keyframe", None),
    ("timeline.editor.nextKey", "Next keyframe", None),
];

/// Native observed labels, defaults and grouping for the searchable setup/reference page.
pub fn capcut_reference_rows() -> Vec<(String, String, String)> {
    crate::capcut_shortcut_reference::ROWS.iter().map(|(category, action, binding)| (category.to_string(), action.to_string(), binding.to_string())).collect()
}

/// PhotoStore mapping or explicit disposition for one row in the observed native reference.
pub fn capcut_reference_mapping(action: &str, binding: &str) -> String {
    if binding == "Unassigned" {
        let service_actions = [
            "Generate captions",
            "Don’t isolate voice",
            "Keep vocal",
            "Remove vocal",
            "Sync video and audio",
            "Transcript",
            "Image to video",
            "Create multi-camera clip",
            "Voiceover",
        ];
        return if service_actions.contains(&action) {
            "No native default · external service workflow outside this timeline scope".into()
        } else {
            "No native default · no shortcut assigned; command availability is separate".into()
        };
    }
    let mapping = match action {
        "Split" => "Selected clips · timeline.editor.split",
        "Split all" => "Eligible clips · timeline.editor.splitAll",
        "Select mode" => "Timeline selection mode · A",
        "Split mode" => "Timeline razor mode · B",
        "Select leftward" => "Previous clip · [",
        "Select rightward" => "Next clip · ]",
        "Main track magnet" => "Persisted Main-track edge magnet · P",
        "Auto snapping" => "Clip-edge, marker and playhead snapping · N",
        "Linkage switch" => "Linked selection mode · Shift+Backtick (tilde alias adapted)",
        "Preview axis switch" => "Preview skimming mode · S",
        "Zoom in" => "Timeline zoom · Cmd++ / Cmd+=",
        "Zoom out" => "Timeline zoom · Cmd+-",
        "Zoom in/Zoom out" => "Cmd+wheel anchored timeline zoom",
        "Up/Down" => "Vertical timeline scroll",
        "Left/Right" => "Alt+wheel horizontal timeline pan",
        "Activate/Deactivate clip" => "Enable or disable selected clip · V",
        "Extract/Restore audio" => "Extract or remove linked source audio · Ctrl+Shift+S",
        "Add marker" => "Timeline marker · M",
        "Add marker with new color" => "Colored timeline marker · Alt+M",
        "Go to next marker" => "Seek next marker · Shift+M",
        "Go to previous marker" => "Seek previous marker · Alt+Shift+M",
        "Previous frame" => "Previous frame · Left",
        "Next frame" => "Next frame · Right",
        "Locate to first frame" => "Seek start · Home",
        "Locate to last frame" => "Seek end · End",
        "Last cut point" => "Previous edit edge · Up",
        "Next cut point" => "Next edit edge · Down",
        "In point" => "Set work-area start · I",
        "Out point" => "Set work-area end · O",
        "Select by clip" => "Set work area to selected clips · Shift+X",
        "Deselect" => "Clear work area and selection · Alt+X",
        "Group" => "Group selected layers · Cmd+G",
        "Ungroup" => "Ungroup layers · Cmd+Shift+G",
        "Show speed panel" => "Selected clip speed inspector · Cmd+R",
        "Curve speed" => "Variable speed curve editor · Shift+B",
        "Create compound clip (subproject)" => "Convert selected layer to Smart Object · Alt+G",
        "Undo compound clip (subproject)" => "Convert Smart Object to layers · Alt+Shift+G",
        "Volume +0.1dB" => "Selected audio volume · Cmd+. (linear volume conversion)",
        "Volume -0.1dB" => "Selected audio volume · Cmd+, (linear volume conversion)",
        "In" => "Edit Smart Object contents · Shift+Enter (label semantics are uncertain)",
        "Shuttle left" => "Reverse shuttle · Cmd+P compatibility alias",
        "Shuttle stop" => "Stop shuttle · K",
        "Shuttle right" => "Forward shuttle · L",
        "Delete left" => "Trim selected clip to playhead · Q",
        "Delete right" => "Trim selected clip to playhead · W",
        "Show/Hide keyframe panel" => "Selected clip keyframe pane · Alt+K",
        "Add keyframe on keyframe panel" => "Add key at pointer · Shift+Click",
        "Add keyframe" => "Add keys to enabled properties · Alt+Shift+K",
        "Step forward many frames" => "Previous 10 frames · Shift+Left (intuitive PhotoStore arrow direction)",
        "Step back many frames" => "Next 10 frames · Shift+Right (intuitive PhotoStore arrow direction)",
        "Zoom to fit timeline" => "Fit timeline · Shift+Z",
        "Play/Pause" => "Timeline transport · Space",
        "Enter/Exit full screen" | "Full screen" => "Existing native fullscreen action · Cmd+Shift+F / Cmd+Ctrl+F",
        "Cancel player alignment" => "Preview canvas alignment cancel · hold Cmd",
        "Zoom in player" => "Canvas preview zoom · Alt+Shift++",
        "Zoom out player" => "Canvas preview zoom · Alt+Shift+-",
        "Zoom to fit player" => "Fit canvas preview · Alt+Shift+Z",
        "Show/hide rulers" => "Toggle canvas rulers · Cmd+Alt+R",
        "Show/hide guides" => "Toggle canvas guides · Cmd+;",
        "Copy" => "Contextual timeline clip/key clipboard · Cmd+C",
        "Cut" => "Contextual timeline clip/key clipboard · Cmd+X",
        "Paste" => "Contextual timeline clip/key clipboard · Cmd+V",
        "Copy attributes" => "Selected clip appearance/animation clipboard · Cmd+Shift+C",
        "Paste attributes" => "Apply copied clip attributes · Cmd+Shift+V",
        "Delete" => "Selected timeline clips or keys · Backspace/Delete",
        "Undo" => "Timeline edit undo · Cmd+Z",
        "Reset" => "Timeline edit redo adaptation · Cmd+Shift+Z",
        "Import media" => "Existing embedded media import · Cmd+I",
        "Export" => "Existing video render command · Cmd+E",
        "New project" => "PhotoStore File > New · Cmd+N",
        "Switch material panel" => "Current-document Media bin · Tab",
        "Back to edit" => "Release timeline shortcut ownership · Cmd+Alt+Q",
        "Quit" => "Existing application File > Exit · Cmd+Q",
        "Exit full screen" => "Existing Escape handling; also dismisses active editing UI",
        "Resize the text box from the center" => "Paragraph text box center resize · Alt+Drag",
        _ => "Observed binding has no verified PhotoStore equivalent",
    };
    mapping.into()
}

fn scoped_timeline_context(app: &PhotoStoreApp, ctx: &egui::Context) -> bool {
    let owner = ctx.data_mut(|data| data.get_persisted::<u64>(egui::Id::new("timeline-scoped-context")));
    owner.is_some_and(|doc| {
        app.session
            .active()
            .is_some_and(|s| s.doc.id.0 == doc && s.doc.timeline.as_ref().is_some_and(|tl| matches!(tl.mode, photostore_doc::TimelineMode::Video)))
    })
}

fn dispatch_timeline_shortcuts(app: &mut PhotoStoreApp, ctx: &egui::Context) -> bool {
    use crate::shortcut_dispatch::Focus;
    if !scoped_timeline_context(app, ctx) || Focus::of(ctx) != Focus::None || app.ui.timeline.open == false {
        return false;
    }
    if let Some((doc, selected_layers)) = app.session.active().map(|session| (session.doc.id, session.selected_layers.clone())) {
        let keypane_id = egui::Id::new(("timeline-key-pane", doc.0));
        let keypane_open = ctx.data_mut(|data| data.get_persisted::<bool>(keypane_id)).unwrap_or(false);
        let key_selection = !crate::video_clip_ui::selected_keys(ctx, doc.0).is_empty()
            || ctx.data(|data| data.get_temp::<crate::video_clip_ui::KeyIdentity>(egui::Id::new(("timeline-selected-key", doc.0)))).is_some();
        let audio_selection = crate::audio_ui::selected_clip_ids(ctx, doc);
        let has_clip_selection = !selected_layers.is_empty() || !audio_selection.is_empty();
        if (key_selection || has_clip_selection || keypane_open) && consume(ctx, &KeyboardShortcut::new(Modifiers::NONE, Key::Escape)) {
            if key_selection {
                crate::video_clip_ui::clear_key_selection(ctx, doc.0);
            } else if has_clip_selection {
                if let Err(error) = app.run("timeline.editor.select", json!({"document":doc.0,"layers":[],"audio":[]})) {
                    app.ui.status = error;
                    app.ui.status_error = true;
                }
                crate::video_clip_ui::clear_selection(ctx, doc);
                crate::audio_ui::clear_selection(ctx, doc);
            } else {
                ctx.data_mut(|data| data.insert_persisted(keypane_id, false));
                crate::video_clip_ui::clear_key_selection(ctx, doc.0);
            }
            return true;
        }
    }
    for (id, _, binding) in TIMELINE_SHORTCUTS {
        let default = binding.map(str::to_string);
        let keys = app.session.prefs().shortcuts_for(id, default.as_deref());
        for raw in keys {
            let Some(shortcut) = parse(&raw) else { continue };
            if !consume(ctx, &shortcut) {
                continue;
            }
            let Some((document, active_layer, selected_layers)) = app.session.active().map(|st| (st.doc.clone(), st.active_layer, st.selected_layers.clone()))
            else {
                return true;
            };
            let doc = document.id.0;
            let layer = active_layer.map(|id| id.0);
            let frame = document.timeline.as_ref().map_or(0, |t| t.current);
            let selected_audio = crate::audio_ui::selected_clip_ids(ctx, document.id);
            let mut params = serde_json::json!({"document":doc});
            let mut command: Option<&str> = None;
            let selected_key = ctx.data(|d| d.get_temp::<crate::video_clip_ui::KeyIdentity>(egui::Id::new(("timeline-selected-key", doc))));
            let selected_keys = crate::video_clip_ui::selected_keys(ctx, doc);
            let key_refs: Vec<_> = selected_keys.iter().map(|key| serde_json::json!({"layer":key.layer,"property":key.property,"frame":key.frame})).collect();
            match *id {
                "timeline.editor.delete" => {
                    if !selected_keys.is_empty() {
                        command = Some("timeline.editor.key.delete");
                        params["keys"] = serde_json::json!(key_refs);
                    } else {
                        command = Some("timeline.editor.delete");
                        params["layers"] = serde_json::json!(selected_layers.iter().map(|layer| layer.0).collect::<Vec<_>>());
                        params["audio"] = serde_json::json!(selected_audio);
                    }
                }
                "timeline.editor.copy" | "timeline.editor.cut" => {
                    if !selected_keys.is_empty() {
                        let keys = serde_json::json!(key_refs);
                        if let Err(error) = app.run("timeline.editor.key.copy", serde_json::json!({"document":doc,"keys":keys.clone()})) {
                            app.ui.status = error;
                            app.ui.status_error = true;
                            return true;
                        }
                        if *id == "timeline.editor.cut" {
                            if let Err(error) = app.run("timeline.editor.key.delete", serde_json::json!({"document":doc,"keys":keys})) {
                                app.ui.status = error;
                                app.ui.status_error = true;
                            }
                        }
                        return true;
                    }
                    command = Some(id);
                    params["layers"] = serde_json::json!(selected_layers.iter().map(|layer| layer.0).collect::<Vec<_>>());
                    params["audio"] = serde_json::json!(selected_audio);
                }
                "timeline.editor.copyAttributes" => {
                    if let Some(source) = selected_layers.first() {
                        command = Some("timeline.editor.copyAttributes");
                        params["layer"] = serde_json::json!(source.0);
                    }
                }
                "timeline.editor.pasteAttributes" => {
                    if !selected_layers.is_empty() {
                        command = Some("timeline.editor.pasteAttributes");
                        params["layers"] = serde_json::json!(selected_layers.iter().map(|layer| layer.0).collect::<Vec<_>>());
                    }
                }
                "timeline.editor.paste" => {
                    if !selected_keys.is_empty() || selected_key.is_some() {
                        command = Some("timeline.editor.key.paste");
                        params["frame"] = serde_json::json!(frame);
                    } else {
                        command = Some("timeline.editor.paste");
                        params["frame"] = serde_json::json!(frame);
                    }
                }
                "timeline.editor.splitSelected" => {
                    if !selected_layers.is_empty() {
                        command = Some("timeline.editor.split");
                        params["layers"] = serde_json::json!(selected_layers.iter().map(|layer| layer.0).collect::<Vec<_>>());
                        params["frame"] = serde_json::json!(frame);
                    } else if let Some(clip) = selected_audio.first()
                        && let Some(timeline) = document.timeline.as_ref()
                        && let Some(audio) = timeline.audio_tracks.iter().flat_map(|track| track.clips.iter()).find(|audio| audio.id == *clip)
                    {
                        command = Some("timeline.splitAudioClip");
                        params = serde_json::json!({"document":doc,"clip":clip,"timeSeconds":timeline.frame_start_seconds(frame)});
                    }
                }
                "timeline.editor.trimLeftToPlayhead" | "timeline.editor.trimRightToPlayhead" => {
                    if let Some(clip_id) = selected_audio.first()
                        && let Some(timeline) = document.timeline.as_ref()
                        && let Some(clip) = timeline.audio_tracks.iter().flat_map(|track| track.clips.iter()).find(|clip| clip.id == *clip_id)
                    {
                        let playhead = timeline.frame_start_seconds(frame);
                        let mut params = serde_json::json!({"document":doc,"clip":clip.id});
                        if *id == "timeline.editor.trimLeftToPlayhead" && playhead > clip.start_seconds && playhead < clip.start_seconds + clip.duration_seconds
                        {
                            params["startSeconds"] = serde_json::json!(playhead);
                            params["trimInSeconds"] = serde_json::json!(clip.trim_in_seconds + (playhead - clip.start_seconds) * clip.speed);
                            params["durationSeconds"] = serde_json::json!(clip.start_seconds + clip.duration_seconds - playhead);
                        } else if *id == "timeline.editor.trimRightToPlayhead"
                            && playhead > clip.start_seconds
                            && playhead < clip.start_seconds + clip.duration_seconds
                        {
                            params["durationSeconds"] = serde_json::json!(playhead - clip.start_seconds);
                        } else {
                            return true;
                        }
                        if let Err(error) = app.run("timeline.setAudioClip", params) {
                            app.ui.status = error;
                            app.ui.status_error = true;
                        }
                        return true;
                    }
                    let clips: Vec<_> = selected_layers
                        .iter()
                        .filter(|layer| document.timeline.as_ref().is_some_and(|t| t.layer_ranges.iter().any(|range| range.layer == **layer)))
                        .map(|layer| layer.0)
                        .collect();
                    if !clips.is_empty() {
                        command = Some(if *id == "timeline.editor.trimLeftToPlayhead" { "timeline.editor.trimLeft" } else { "timeline.editor.trimRight" });
                        params["layers"] = serde_json::json!(clips);
                        params["frame"] = serde_json::json!(frame);
                    }
                }
                "timeline.editor.splitAll" => {
                    command = Some("timeline.editor.splitAll");
                    params["frame"] = serde_json::json!(frame);
                }
                "timeline.editor.selectMode" => {
                    ctx.data_mut(|d| d.insert_persisted(egui::Id::new(("timeline-razor-mode", doc)), false));
                }
                "timeline.editor.razorMode" => {
                    ctx.data_mut(|d| d.insert_persisted(egui::Id::new(("timeline-razor-mode", doc)), true));
                }
                "timeline.editor.undo" => {
                    command = Some("edit.undo");
                }
                "timeline.editor.redo" => {
                    command = Some("edit.redo");
                }
                "timeline.editor.playPause" => {
                    app.ui.timeline.playing = !app.ui.timeline.playing;
                    let mut rt = std::mem::take(&mut app.timeline_playback);
                    if app.ui.timeline.playing {
                        rt.play(app, ctx.input(|i| i.time));
                    } else {
                        rt.stop();
                    }
                    app.timeline_playback = rt;
                }
                "timeline.editor.shuttleReverse" | "timeline.editor.shuttleReverseAlias" | "timeline.editor.shuttleForward" | "timeline.editor.shuttleStop" => {
                    let direction = match *id {
                        "timeline.editor.shuttleReverse" | "timeline.editor.shuttleReverseAlias" => -1,
                        "timeline.editor.shuttleForward" => 1,
                        _ => 0,
                    };
                    app.ui.timeline.play_direction = direction;
                    app.ui.timeline.playing = direction != 0;
                    let mut runtime = std::mem::take(&mut app.timeline_playback);
                    if direction == 0 {
                        runtime.stop();
                    } else {
                        runtime.shuttle(app, ctx.input(|input| input.time), direction);
                    }
                    app.timeline_playback = runtime;
                }
                "timeline.editor.framePrevious" | "timeline.editor.stepLeftAlias" => {
                    command = Some("timeline.previousFrame");
                }
                "timeline.editor.frameNext" | "timeline.editor.stepRightAlias" => {
                    command = Some("timeline.nextFrame");
                }
                "timeline.editor.framePreviousMany" => {
                    command = Some("timeline.setFrame");
                    params["frame"] = serde_json::json!(frame.saturating_sub(10));
                }
                "timeline.editor.frameNextMany" => {
                    command = Some("timeline.setFrame");
                    params["frame"] =
                        serde_json::json!(frame.saturating_add(10).min(document.timeline.as_ref().map_or(frame, |t| t.duration.saturating_sub(1))));
                }
                "timeline.editor.firstFrame" => {
                    command = Some("timeline.setFrame");
                    params["frame"] = serde_json::json!(0);
                }
                "timeline.editor.lastFrame" => {
                    command = Some("timeline.setFrame");
                    params["frame"] = serde_json::json!(document.timeline.as_ref().map_or(0, |t| t.duration.saturating_sub(1)));
                }
                "timeline.editor.magnet" => {
                    command = Some("timeline.editor.setModes");
                    params["mainTrackMagnet"] = serde_json::json!(!document.timeline.as_ref().is_some_and(|t| t.main_track_magnet));
                }
                "timeline.editor.snapping" => {
                    command = Some("timeline.editor.setModes");
                    params["snapping"] = serde_json::json!(!document.timeline.as_ref().is_some_and(|t| t.snapping));
                }
                "timeline.editor.linkedSelection" => {
                    command = Some("timeline.editor.setModes");
                    params["linkedSelection"] = serde_json::json!(!document.timeline.as_ref().is_some_and(|t| t.linked_selection));
                }
                "timeline.editor.previewSkimming" => {
                    command = Some("timeline.editor.setModes");
                    params["previewSkimming"] = serde_json::json!(!document.timeline.as_ref().is_some_and(|t| t.preview_skimming));
                }
                "timeline.editor.enableClip" => {
                    if let Some(clip) = selected_audio.first()
                        && let Some(audio) = document
                            .timeline
                            .as_ref()
                            .and_then(|timeline| timeline.audio_tracks.iter().flat_map(|track| track.clips.iter()).find(|audio| audio.id == *clip))
                    {
                        command = Some("timeline.setAudioClip");
                        params["clip"] = serde_json::json!(clip);
                        params["muted"] = serde_json::json!(!audio.muted);
                    } else if let Some(layer) = layer {
                        command = Some("timeline.editor.enableClip");
                        params["layers"] = serde_json::json!([layer]);
                        params["enabled"] = serde_json::json!(
                            !document.timeline.as_ref().and_then(|t| t.layer_ranges.iter().find(|r| r.layer.0 == layer)).is_some_and(|r| r.enabled)
                        );
                    }
                }
                "timeline.editor.markerAdd" => {
                    command = Some("timeline.editor.marker.add");
                    params["frame"] = serde_json::json!(frame);
                }
                "timeline.editor.markerAddColor" => {
                    command = Some("timeline.editor.marker.add");
                    params["frame"] = serde_json::json!(frame);
                    params["color"] = serde_json::json!([255, 196, 64]);
                }
                "timeline.editor.markerNext" | "timeline.editor.markerPrevious" => {
                    let markers = document.timeline.as_ref().map(|t| t.markers.as_slice()).unwrap_or(&[]);
                    let target = if *id == "timeline.editor.markerNext" {
                        markers.iter().filter(|m| m.frame > frame).min_by_key(|m| m.frame)
                    } else {
                        markers.iter().filter(|m| m.frame < frame).max_by_key(|m| m.frame)
                    };
                    if let Some(marker) = target {
                        command = Some("timeline.setFrame");
                        params["frame"] = serde_json::json!(marker.frame);
                    }
                }
                "timeline.editor.previousEdit" | "timeline.editor.nextEdit" => {
                    let ranges = document.timeline.as_ref().map(|t| t.layer_ranges.as_slice()).unwrap_or(&[]);
                    let target = if *id == "timeline.editor.nextEdit" {
                        ranges.iter().flat_map(|r| [r.start, r.end]).filter(|f| *f > frame).min()
                    } else {
                        ranges.iter().flat_map(|r| [r.start, r.end]).filter(|f| *f < frame).max()
                    };
                    if let Some(target) = target {
                        command = Some("timeline.setFrame");
                        params["frame"] = serde_json::json!(target);
                    }
                }
                "timeline.editor.workAreaIn" | "timeline.editor.workAreaOut" => {
                    let t = document.timeline.as_ref();
                    command = Some("timeline.editor.workArea.set");
                    params["start"] = serde_json::json!(if *id == "timeline.editor.workAreaIn" { frame } else { t.map_or(0, |t| t.work_start) });
                    params["end"] = serde_json::json!(if *id == "timeline.editor.workAreaOut" {
                        frame.saturating_add(1)
                    } else {
                        t.map_or(frame.saturating_add(1), |t| t.work_end)
                    });
                }
                "timeline.editor.zoomIn" | "timeline.editor.zoomInEquals" => {
                    app.ui.timeline.zoom = (app.ui.timeline.zoom * 1.25).clamp(0.25, 16.0);
                }
                "timeline.editor.zoomOut" => {
                    app.ui.timeline.zoom = (app.ui.timeline.zoom / 1.25).clamp(0.25, 16.0);
                }
                "timeline.editor.zoomFit" => {
                    app.ui.timeline.zoom = 1.0;
                    ctx.data_mut(|data| data.insert_temp(egui::Id::new(("timeline-time-offset", doc)), 0.0_f32));
                }
                "timeline.editor.keyPane" => {
                    let key = egui::Id::new(("timeline-key-pane", doc));
                    let value = ctx.data_mut(|d| d.get_persisted::<bool>(key)).unwrap_or(false);
                    ctx.data_mut(|d| d.insert_persisted(key, !value));
                }
                "timeline.editor.materialBin" => {
                    app.ui.timeline.media_open = !app.ui.timeline.media_open;
                }
                "timeline.editor.deselect" => {
                    if let Some(timeline) = document.timeline.as_ref()
                        && let Err(error) =
                            app.run("timeline.editor.workArea.set", serde_json::json!({"document":doc,"start":0,"end":timeline.duration.max(1)}))
                    {
                        app.ui.status = error;
                        app.ui.status_error = true;
                    }
                    if let Err(error) = app.with_document(document.id, |app| app.run("select.deselectLayers", serde_json::json!({}))) {
                        app.ui.status = error;
                        app.ui.status_error = true;
                    }
                    crate::audio_ui::clear_selection(ctx, document.id);
                    return true;
                }
                "timeline.editor.group" => {
                    if selected_layers.len() > 1 {
                        command = Some("layer.groupLayers");
                    }
                }
                "timeline.editor.ungroup" => {
                    command = Some("layer.ungroupLayers");
                }
                "timeline.editor.makeCompound" => {
                    command = Some("layer.smartObjects.convertToSmartObject");
                }
                "timeline.editor.unpackCompound" => {
                    command = Some("layer.smartObjects.convertToLayers");
                }
                "timeline.editor.enterContents" => {
                    if let Some(layer) = layer {
                        command = Some("layer.smartObjects.editContents");
                        params["layer"] = serde_json::json!(layer);
                    }
                }
                "timeline.editor.speedPanel" | "timeline.editor.speedCurve" => {
                    app.ui.panels.properties = true;
                    ctx.data_mut(|data| data.insert_temp(egui::Id::new(("timeline-speed-panel-focus", doc)), true));
                }
                "timeline.editor.audioVolumeUp" | "timeline.editor.audioVolumeDown" => {
                    if let Some(clip_id) = crate::audio_ui::selected_clip_id(ctx, document.id)
                        && let Some(clip) = document
                            .timeline
                            .as_ref()
                            .and_then(|timeline| timeline.audio_tracks.iter().flat_map(|track| track.clips.iter()).find(|clip| clip.id == clip_id))
                    {
                        let factor = 10.0_f64.powf(if *id == "timeline.editor.audioVolumeUp" { 0.1 } else { -0.1 } / 20.0);
                        command = Some("timeline.setAudioClip");
                        params = serde_json::json!({"document":doc,"clip":clip_id,"volume":(clip.volume*factor).clamp(0.0,4.0)});
                    }
                }
                "timeline.editor.selectLeftward" | "timeline.editor.selectRightward" => {
                    let ranges = document
                        .timeline
                        .as_ref()
                        .map(|timeline| {
                            let mut ranges: Vec<_> = timeline.layer_ranges.iter().collect();
                            ranges.sort_by_key(|range| (range.start, range.end));
                            ranges
                        })
                        .unwrap_or_default();
                    if let Some(index) = ranges.iter().position(|range| Some(range.layer) == active_layer) {
                        let target =
                            if *id == "timeline.editor.selectLeftward" { index.checked_sub(1).and_then(|i| ranges.get(i)) } else { ranges.get(index + 1) };
                        if let Some(target) = target {
                            let _ = app.with_document(document.id, |app| {
                                app.run("timeline.editor.select", serde_json::json!({"document":doc,"layers":[target.layer.0]}))
                            });
                            crate::audio_ui::clear_selection(ctx, document.id);
                        }
                    }
                    return true;
                }
                "timeline.editor.extractAudio" => {
                    command = Some("timeline.editor.extractAudio");
                    if !selected_layers.is_empty() {
                        params["layer"] = serde_json::json!(selected_layers[0].0);
                    }
                }
                "timeline.editor.workAreaSelection" => {
                    let Some(timeline) = document.timeline.as_ref() else { return true };
                    let mut starts: Vec<usize> =
                        timeline.layer_ranges.iter().filter(|range| selected_layers.contains(&range.layer)).map(|range| range.start).collect();
                    let mut ends: Vec<usize> =
                        timeline.layer_ranges.iter().filter(|range| selected_layers.contains(&range.layer)).map(|range| range.end).collect();
                    for clip_id in &selected_audio {
                        if let Some(clip) = timeline.audio_tracks.iter().flat_map(|track| track.clips.iter()).find(|clip| clip.id == *clip_id) {
                            let fps = f64::from(timeline.fps.max(0.001));
                            starts.push((clip.start_seconds * fps).floor().max(0.0) as usize);
                            ends.push(((clip.start_seconds + clip.duration_seconds) * fps).ceil().max(1.0) as usize);
                        }
                    }
                    if starts.is_empty()
                        && let Some(range) =
                            timeline.layer_ranges.iter().filter(|range| range.start <= frame && frame < range.end).max_by_key(|range| range.start)
                    {
                        starts.push(range.start);
                        ends.push(range.end);
                    }
                    if let (Some(start), Some(end)) = (starts.into_iter().min(), ends.into_iter().max()) {
                        let end = end.max(start.saturating_add(1)).min(timeline.duration.max(1));
                        if let Err(error) = app.run("timeline.editor.workArea.set", serde_json::json!({"document":doc,"start":start.min(end-1),"end":end})) {
                            app.ui.status = error;
                            app.ui.status_error = true;
                        }
                    }
                    return true;
                }
                "timeline.editor.importMedia" => {
                    if let Err(error) = crate::menus::invoke(app, ctx, "file.placeEmbedded", serde_json::json!({})) {
                        app.ui.status = error;
                        app.ui.status_error = true;
                    }
                    return true;
                }
                "timeline.editor.exportVideo" => {
                    if let Some(Err(error)) = crate::movie_ui::invoke(app, ctx, "file.export.renderVideo", &serde_json::json!({})) {
                        app.ui.status = error;
                        app.ui.status_error = true;
                    }
                    return true;
                }
                "timeline.editor.backToEdit" => {
                    ctx.data_mut(|data| data.remove::<u64>(egui::Id::new("timeline-scoped-context")));
                    return true;
                }
                "timeline.editor.playerFullscreen" | "timeline.editor.playerFullscreenLegacy" => {
                    if let Err(error) = crate::menus::invoke(app, ctx, "view.screenMode.fullScreen", serde_json::json!({})) {
                        app.ui.status = error;
                        app.ui.status_error = true;
                    }
                    return true;
                }
                "timeline.editor.playerZoomIn" | "timeline.editor.playerZoomOut" | "timeline.editor.playerZoomFit" => {
                    let menu_command = match *id {
                        "timeline.editor.playerZoomIn" => "view.zoomIn",
                        "timeline.editor.playerZoomOut" => "view.zoomOut",
                        _ => "view.fitOnScreen",
                    };
                    if let Err(error) = crate::menus::invoke(app, ctx, menu_command, serde_json::json!({})) {
                        app.ui.status = error;
                        app.ui.status_error = true;
                    }
                    return true;
                }
                "timeline.editor.playerRulers" | "timeline.editor.playerGuides" => {
                    let menu_command = if *id == "timeline.editor.playerRulers" { "view.rulers" } else { "view.show.guides" };
                    if let Err(error) = crate::menus::invoke(app, ctx, menu_command, serde_json::json!({})) {
                        app.ui.status = error;
                        app.ui.status_error = true;
                    }
                    return true;
                }
                "timeline.editor.addKeyframe" => {
                    if let Some(active) = active_layer.and_then(|id| document.layer(id)) {
                        let mut first_error = None;
                        if let Some(animation) = active.position_animation.as_ref().filter(|animation| animation.enabled) {
                            let value = animation.sampled_value(frame);
                            if let Err(error) = app.run(
                                "timeline.setPositionKeyframe",
                                serde_json::json!({"document":doc,"layer":active.id.0,"frame":frame,"x":value.0,"y":value.1}),
                            ) {
                                first_error = Some(error);
                            }
                        }
                        if let Some(animation) = active.opacity_animation.as_ref().filter(|animation| animation.enabled) {
                            let value = animation.sampled_value(frame);
                            if let Err(error) =
                                app.run("timeline.setOpacityKeyframe", serde_json::json!({"document":doc,"layer":active.id.0,"frame":frame,"value":value}))
                            {
                                first_error.get_or_insert(error);
                            }
                        }
                        if let Some(animation) = active.transform_animation.as_ref().filter(|animation| animation.enabled) {
                            let value = animation.sampled_value(frame);
                            if let Err(error) = app.run("timeline.setTransformKeyframe", serde_json::json!({"document":doc,"layer":active.id.0,"frame":frame,"scaleX":value.scale_x,"scaleY":value.scale_y,"rotation":value.rotation})) { first_error.get_or_insert(error); }
                        }
                        if let Some(error) = first_error {
                            app.ui.status = error;
                            app.ui.status_error = true;
                        }
                    }
                    return true;
                }
                "timeline.editor.keyEaseIn"
                | "timeline.editor.keyEaseOut"
                | "timeline.editor.keyEaseInOut"
                | "timeline.editor.keyLinear"
                | "timeline.editor.keyHold" => {
                    if selected_keys.is_empty() {
                        app.ui.status = "Select one or more keyframes first".into();
                        app.ui.status_error = false;
                        return true;
                    }
                    let easing = match *id {
                        "timeline.editor.keyEaseIn" => "ease_in",
                        "timeline.editor.keyEaseOut" => "ease_out",
                        "timeline.editor.keyEaseInOut" => "ease_in_out",
                        "timeline.editor.keyHold" => "hold",
                        _ => "linear",
                    };
                    command = Some("timeline.editor.key.ease");
                    params["keys"] = serde_json::json!(key_refs);
                    params["easing"] = serde_json::json!(easing);
                }
                "timeline.editor.previousKey" | "timeline.editor.nextKey" => {
                    let base = selected_keys.first().map_or(frame, |key| key.frame);
                    let mut candidates = Vec::new();
                    let refs = if selected_keys.is_empty() {
                        let mut refs = Vec::new();
                        if let Some(active) = active_layer.and_then(|id| document.layer(id)) {
                            refs.extend(active.position_animation.iter().flat_map(|animation| {
                                animation.keys.iter().map(|key| crate::video_clip_ui::KeyIdentity {
                                    layer: active.id.0,
                                    property: "position",
                                    frame: key.frame,
                                })
                            }));
                            refs.extend(active.opacity_animation.iter().flat_map(|animation| {
                                animation.keys.iter().map(|key| crate::video_clip_ui::KeyIdentity { layer: active.id.0, property: "opacity", frame: key.frame })
                            }));
                            refs.extend(active.transform_animation.iter().flat_map(|animation| {
                                animation.keys.iter().map(|key| crate::video_clip_ui::KeyIdentity {
                                    layer: active.id.0,
                                    property: "transform",
                                    frame: key.frame,
                                })
                            }));
                        }
                        refs
                    } else {
                        selected_keys.clone()
                    };
                    for reference in refs {
                        let Some(target) = document.layer(photostore_doc::LayerId(reference.layer)) else { continue };
                        match reference.property {
                            "position" => candidates.extend(target.position_animation.iter().flat_map(|a| a.keys.iter().map(|key| key.frame))),
                            "opacity" => candidates.extend(target.opacity_animation.iter().flat_map(|a| a.keys.iter().map(|key| key.frame))),
                            "transform" => candidates.extend(target.transform_animation.iter().flat_map(|a| a.keys.iter().map(|key| key.frame))),
                            _ => {}
                        }
                    }
                    let target = if *id == "timeline.editor.nextKey" {
                        candidates.into_iter().filter(|key| *key > base).min()
                    } else {
                        candidates.into_iter().filter(|key| *key < base).max()
                    };
                    if let Some(target) = target {
                        command = Some("timeline.setFrame");
                        params["frame"] = serde_json::json!(target);
                    }
                }
                _ => {}
            }
            if let Some(command) = command {
                if let Err(error) = app.run(command, params) {
                    app.ui.status = error;
                    app.ui.status_error = true;
                }
            }
            return true;
        }
    }
    false
}

/// Stable, language independent ids for the tool groups shown in Keyboard Shortcuts.
pub fn tool_id(tool: Tool) -> &'static str {
    match tool {
        Tool::Move => "tools.tool.move",
        Tool::RectMarquee => "tools.tool.rectMarquee",
        Tool::EllipseMarquee => "tools.tool.ellipseMarquee",
        Tool::Lasso => "tools.tool.lasso",
        Tool::PolygonLasso => "tools.tool.polygonLasso",
        Tool::MagneticLasso => "tools.tool.magneticLasso",
        Tool::MagicWand => "tools.tool.magicWand",
        Tool::QuickSelection => "tools.tool.quickSelection",
        Tool::ObjectSelection => "tools.tool.objectSelection",
        Tool::Brush => "tools.tool.brush",
        Tool::Pencil => "tools.tool.pencil",
        Tool::MixerBrush => "tools.tool.mixerBrush",
        Tool::Eraser => "tools.tool.eraser",
        Tool::BackgroundEraser => "tools.tool.backgroundEraser",
        Tool::MagicEraser => "tools.tool.magicEraser",
        Tool::Gradient => "tools.tool.gradient",
        Tool::PaintBucket => "tools.tool.paintBucket",
        Tool::Type => "tools.tool.type",
        Tool::VerticalType => "tools.tool.verticalType",
        Tool::Hand => "tools.tool.hand",
        Tool::RotateView => "tools.tool.rotateView",
        Tool::Zoom => "tools.tool.zoom",
        Tool::Remove => "tools.tool.remove",
        Tool::SpotHealing => "tools.tool.spotHealing",
        Tool::Healing => "tools.tool.healing",
        Tool::Patch => "tools.tool.patch",
        Tool::ContentAwareMove => "tools.tool.contentAwareMove",
        Tool::RedEye => "tools.tool.redEye",
        Tool::CloneStamp => "tools.tool.cloneStamp",
        Tool::PatternStamp => "tools.tool.patternStamp",
        Tool::HistoryBrush => "tools.tool.historyBrush",
        Tool::Blur => "tools.tool.blur",
        Tool::Sharpen => "tools.tool.sharpen",
        Tool::Smudge => "tools.tool.smudge",
        Tool::Dodge => "tools.tool.dodge",
        Tool::Burn => "tools.tool.burn",
        Tool::Sponge => "tools.tool.sponge",
        Tool::Pen => "tools.tool.pen",
        Tool::PathSelection => "tools.tool.pathSelection",
        Tool::DirectSelection => "tools.tool.directSelection",
        Tool::Rectangle => "tools.tool.rectangle",
        Tool::EllipseShape => "tools.tool.ellipseShape",
        Tool::Triangle => "tools.tool.triangle",
        Tool::Polygon => "tools.tool.polygon",
        Tool::Line => "tools.tool.line",
        Tool::CustomShape => "tools.tool.customShape",
        Tool::Eyedropper => "tools.tool.eyedropper",
        Tool::Ruler => "tools.tool.ruler",
        Tool::Note => "tools.tool.note",
        Tool::Count => "tools.tool.count",
        Tool::Crop => "tools.tool.crop",
        Tool::Slice => "tools.tool.slice",
        Tool::SliceSelect => "tools.tool.sliceSelect",
    }
}

pub fn select_tool_command(app: &mut PhotoStoreApp, id: &str) -> bool {
    let Some(tool) = Tool::ALL.into_iter().find(|t| tool_id(*t) == id) else { return false };
    app.ui.tool = tool;
    true
}

/// With ⇧ held the OS reports the shifted character as the logical key (US layout): ⇧; is `:`.
fn shifted(k: Key) -> Option<Key> {
    Some(match k {
        Key::Equals => Key::Plus,
        Key::Semicolon => Key::Colon,
        Key::OpenBracket => Key::OpenCurlyBracket,
        Key::CloseBracket => Key::CloseCurlyBracket,
        Key::Slash => Key::Questionmark,
        Key::Backslash => Key::Pipe,
        Key::Num1 => Key::Exclamationmark,
        _ => return None,
    })
}

/// Does a key press match a shortcut, the way Photoshop matches them?
///
/// - Modifiers match exactly (⌘⌥I is not ⌘I), with ⌘ = Ctrl off the Mac.
/// - `=` also matches `+` (⌘+ with ⇧ on a US layout, the numpad `+`, the `+` key of Nordic and
///   German layouts): Photoshop zooms in on both.
/// - Delete also matches Backspace (the Mac's "delete" key), and Backspace matches Delete (⌥⌫
///   and ⌥Delete both fill on Windows).
/// - A ⇧ shortcut on a punctuation key matches the shifted character (⌘⇧; arrives as ⌘`:`).
pub fn key_matches(sc: &KeyboardShortcut, key: Key, mods: Modifiers) -> bool {
    let k = sc.logical_key;
    let direct = key == k || (k == Key::Delete && key == Key::Backspace) || (k == Key::Backspace && key == Key::Delete);
    let plus = k == Key::Equals && key == Key::Plus;
    let via_shift = sc.modifiers.shift && shifted(k) == Some(key);
    if !(direct || plus || via_shift) {
        return false;
    }
    // Typing `+` (or another shifted symbol) may need ⇧ on this layout: don't require it absent.
    let shifted_symbol =
        matches!(key, Key::Plus | Key::Colon | Key::OpenCurlyBracket | Key::CloseCurlyBracket | Key::Questionmark | Key::Pipe | Key::Exclamationmark);
    let shift_ok = mods.shift == sc.modifiers.shift || (shifted_symbol && !sc.modifiers.shift);
    mods.alt == sc.modifiers.alt && shift_ok && mods.cmd_ctrl_matches(sc.modifiers)
}

/// Consume a press of `sc` (see [`key_matches`]); true when one was found.
pub fn consume(ctx: &egui::Context, sc: &KeyboardShortcut) -> bool {
    ctx.input_mut(|i| {
        let before = i.events.len();
        i.events.retain(|e| !matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. } if key_matches(sc, *key, *modifiers)));
        i.events.len() != before
    })
}

/// View navigation that Photoshop keeps live while a dialog is open (so the image can be judged
/// at any zoom): Zoom In / Out, Fit on Screen, 100%.
pub const NAV_COMMANDS: [&str; 4] = ["view.zoomIn", "view.zoomOut", "view.fitOnScreen", "view.actualPixels"];

/// ⌘+ / ⌘- / ⌘0 / ⌘1 (or their Edit › Keyboard Shortcuts overrides). Returns true when one ran.
fn nav_keys(app: &mut PhotoStoreApp, ctx: &egui::Context) -> bool {
    for id in NAV_COMMANDS {
        let sc = crate::menus::UI_COMMANDS.iter().find(|c| c.0 == id).and_then(|c| parse(app.session.prefs().shortcut(id, c.3)?));
        if let Some(sc) = sc
            && consume(ctx, &sc)
        {
            if crate::menus::is_enabled(app, id) {
                let _ = crate::menus::invoke(app, ctx, id, json!({}));
            }
            return true;
        }
    }
    false
}

/// The key press a clipboard event stands for outside text fields, with `mods` held: Copy, Cut
/// and Paste are ⌘C, ⌘X and ⌘V (with ⇧/⌥ as held). On Windows egui-winit also sends Cut for
/// Shift+Delete, which is Edit › Fill here (#530): a Cut with ⇧ and without Ctrl is that key.
fn clipboard_press(e: &egui::Event, mods: Modifiers, is_windows: bool) -> Option<(Key, Modifiers)> {
    let held = if mods.command { mods } else { Modifiers::COMMAND };
    match e {
        egui::Event::Copy => Some((Key::C, held)),
        egui::Event::Cut if is_windows && mods.shift && !mods.command => Some((Key::Delete, mods)),
        egui::Event::Cut => Some((Key::X, held)),
        egui::Event::Paste(_) => Some((Key::V, held)),
        _ => None,
    }
}

/// egui-winit turns ⌘C / ⌘X / ⌘V into `Event::Copy` / `Cut` / `Paste` and drops the key press, and
/// drops ⌘V entirely when the clipboard holds no text (an image). Outside text fields, give the
/// pixel commands their key presses back: Copy/Cut/Paste become ⌘C/⌘X/⌘V presses (with ⇧/⌥ as
/// held, so ⇧⌘C is Copy Merged), and a ⌘V release without a ⌘V press (a paste event, or the Mac
/// menu's key equivalent) becomes a ⌘V press.
///
/// Ctrl+Insert copies and Shift+Insert pastes: egui-winit maps them on Windows (a Shift+Insert
/// release without a paste event pastes the image, like ⌘V), and we map them here on Linux.
pub fn clipboard_keys(ctx: &egui::Context, typing: bool, raw: &mut egui::RawInput) {
    let press = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
    if cfg!(target_os = "linux") {
        raw.events.retain_mut(|e| {
            let egui::Event::Key { key: Key::Insert, pressed: true, modifiers: m, .. } = *e else { return true };
            match (m.ctrl, m.shift) {
                (true, false) => *e = egui::Event::Copy,
                // Text fields need the clipboard text: egui-winit reads it and sends a Paste event.
                (false, true) if typing => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                    return false;
                }
                (false, true) => *e = press(Key::V, Modifiers::COMMAND),
                _ => {}
            }
            true
        });
    }
    if typing {
        // AppKit consumes native-menu key equivalents before egui-winit can turn them into
        // clipboard events. Recreate the text-edit events for those injected key presses.
        let mut out = Vec::with_capacity(raw.events.len());
        for e in raw.events.drain(..) {
            match &e {
                egui::Event::Key { key: Key::C, pressed: true, modifiers, .. } if modifiers.command => out.push(egui::Event::Copy),
                egui::Event::Key { key: Key::X, pressed: true, modifiers, .. } if modifiers.command => out.push(egui::Event::Cut),
                egui::Event::Key { key: Key::V, pressed: true, modifiers, .. } if modifiers.command => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                }
                _ => out.push(e),
            }
        }
        raw.events = out;
        return;
    }
    let seen = egui::Id::new("pc-paste-key-seen");
    // The modifiers held at each event: last frame's, updated as the events report changes.
    let mut mods = ctx.input(|i| i.modifiers);
    let mut out = Vec::with_capacity(raw.events.len());
    for e in raw.events.drain(..) {
        if let egui::Event::ModifiersChanged(m) | egui::Event::Key { modifiers: m, .. } = &e {
            mods = *m;
        }
        if let Some((key, held)) = clipboard_press(&e, mods, cfg!(target_os = "windows")) {
            if key == Key::V {
                ctx.data_mut(|d| d.insert_temp(seen, true));
            }
            out.push(press(key, held));
            continue;
        }
        match &e {
            // A ⌘V press egui-winit didn't swallow (the Mac menu's key equivalent, Shift+Insert on
            // Linux): its release must not paste a second time (#1638).
            egui::Event::Key { key: Key::V, pressed: true, modifiers, .. } if modifiers.command => {
                ctx.data_mut(|d| d.insert_temp(seen, true));
            }
            // A paste key's release ends that press, with or without ⌘ still held (else a stale
            // mark would swallow the next image paste); without a press it is the paste itself.
            egui::Event::Key { key: key @ (Key::V | Key::Insert), pressed: false, modifiers, .. } => {
                let had_press = ctx.data_mut(|d| d.remove_temp::<bool>(seen)).unwrap_or(false);
                let held = match key {
                    Key::V if modifiers.command => Some(*modifiers),
                    Key::Insert if cfg!(target_os = "windows") && modifiers.shift && !modifiers.ctrl => Some(Modifiers::COMMAND),
                    _ => None,
                };
                if let Some(held) = held
                    && !had_press
                {
                    out.push(press(Key::V, held));
                }
            }
            _ => {}
        }
        out.push(e);
    }
    raw.events = out;
}

pub fn handle(app: &mut PhotoStoreApp, ctx: &egui::Context) {
    // Camera Raw is modal like Photoshop's filter dialog: no application shortcut (Save, Undo,
    // tools) runs beneath it, and it handles its own keys (Y, U, O, S). Unlike the other dialogs
    // below it covers the canvas, so canvas zoom keys are blocked too.
    if app.camera_raw.is_some() {
        return;
    }
    // An open menu owns the keyboard (arrows, ↩, Esc), like a native menu.
    if crate::menu_nav::is_open(ctx) {
        return;
    }
    let symmetry_editing = app.ui.symmetry_transform.is_some();
    crate::symmetry_ui::track(app, ctx);
    if symmetry_editing && (app.ui.symmetry_transform.is_none() || egui::Popup::is_any_open(ctx)) {
        return;
    }
    // Liquify is a full-window custom dialog with focusable sliders. egui can therefore claim
    // keyboard input before the distortion-mode handler below runs. Give Liquify's local
    // shortcuts first refusal (Undo, brush size, tool keys), but never steal keys from text edits.
    if app.distort.liquify.is_some() && !ctx.text_edit_focused() {
        crate::liquify_ui::keys(app, ctx);
    }
    use crate::shortcut_dispatch::{Focus, dispatch_pressed};
    let focus = Focus::of(ctx);
    // Dialogs (and Liquify's panel while one of its controls has focus) keep canvas zoom; a
    // focused text field keeps its keys.
    let liquify_focus = app.distort.liquify.is_some() && focus != Focus::None;
    if liquify_focus || !app.ui.dialogs.is_empty() || app.discard.is_some() {
        if focus != Focus::Text {
            nav_keys(app, ctx);
        }
        return;
    }
    if focus == Focus::Text {
        // A focused field keeps its typing and editing keys; menu shortcuts (⌘J, ⌘S, F7…) still
        // fire, as in Photoshop.
        dispatch_pressed(app, ctx, focus, false);
        return;
    }
    // Liquify / Puppet Warp / Perspective Warp: ↩ commits, Esc cancels.
    if crate::distort_ui::keys(app, ctx) {
        return;
    }
    // Free Transform: ↩ commits, Esc cancels.
    if app.ui.transform.is_some() {
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            crate::transform_tool::commit(app);
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            crate::transform_tool::cancel(app);
            return;
        }
    }
    // Moved pixels still floating: ↩ drops them.
    if focus == Focus::None
        && app.session.active().is_some_and(|st| photostore_engine::float_cmds::floating(st).is_some())
        && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter))
    {
        if let Err(e) = app.run("select.drop", json!({})) {
            app.ui.status = e;
            app.ui.status_error = true;
        }
        return;
    }
    // A Pen path's points are uncommitted gesture state, not History entries (#1466).
    // Intercept Cmd/Ctrl+Z before the normal Edit › Undo shortcut, as well as unmodified
    // Backspace/Delete. Once the last anchor is removed, regular Undo works again.
    if app.ui.pen.as_ref().is_some_and(|pen| !pen.knots.is_empty()) {
        let mods = ctx.input(|i| i.modifiers);
        let command_undo = effective_shortcut(app, "edit.undo", default_shortcut("edit.undo").as_deref())
            .and_then(|shortcut| parse(&shortcut))
            .is_some_and(|shortcut| consume(ctx, &shortcut));
        let remove = !mods.command
            && !mods.ctrl
            && !mods.shift
            && !mods.alt
            && ctx.input_mut(|i| i.consume_key(mods, Key::Backspace) || i.consume_key(mods, Key::Delete));
        if command_undo || remove {
            crate::vector_ui::pen_undo_last_point(app);
            return;
        }
    }
    // Pen path in progress: ↩ and Esc both end it as an open path and keep it, as in Photoshop (#1769).
    if app.ui.pen.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Escape)) {
        crate::vector_ui::pen_commit(app, false);
        return;
    }
    // Magnetic Lasso: ↩ closes, Esc cancels, ⌫ removes a fastening point (not Edit › Clear),
    // [ ] change the detection width (not the brush size).
    if crate::magnetic_lasso_ui::keys(app, ctx) {
        return;
    }
    // While placing polygonal lasso vertices, Backspace/Delete retracts the last point;
    // it must win over the regular Edit › Clear shortcut, which edits document pixels.
    if crate::lasso_ui::waiting_for_vertex(app) {
        let mods = ctx.input(|i| i.modifiers);
        if !mods.command && !mods.ctrl && !mods.shift && ctx.input_mut(|i| i.consume_key(mods, Key::Backspace) || i.consume_key(mods, Key::Delete)) {
            crate::lasso_ui::undo_last_vertex(app);
            return;
        }
    }
    // The Polygonal Lasso tool likewise: ⌫/Delete removes the last vertex (#1229), and never
    // reaches Edit › Clear while a polygon is being drawn.
    if !app.ui.polygon.is_empty() {
        let mods = ctx.input(|i| i.modifiers);
        if !mods.command && !mods.ctrl && !mods.shift && ctx.input_mut(|i| i.consume_key(mods, Key::Backspace) || i.consume_key(mods, Key::Delete)) {
            crate::canvas::polygon_retract(app);
            return;
        }
    }
    // Inline type editing owns navigation and value keys before the focused timeline can claim them.
    let editing = crate::type_tool::handle_keys(app, ctx);
    // A timeline click activates its own keymap without changing the global Photoshop mapping.
    // Text fields, inline text, modal UI, menus, distortion modes and Free Transform have priority.
    if !editing && app.ui.transform.is_none() && dispatch_timeline_shortcuts(app, ctx) {
        return;
    }
    if editing {
        // Inline type edits retain application command shortcuts such as Save and Undo.
        if dispatch_pressed(app, ctx, focus, true) {
            return;
        }
        return;
    }
    // In ordinary canvas context, crop and Move/Transform arrows retain priority over global
    // shortcuts. A focused video timeline already had first refusal above.
    if focus == Focus::None && crate::crop_ui::keys(app, ctx) {
        return;
    }
    if focus == Focus::None && crate::move_mods::arrow_keys(app, ctx) {
        return;
    }
    // Registry, UI and menu-catalogue shortcuts (see [`crate::shortcut_dispatch::bindings`]).
    if dispatch_pressed(app, ctx, focus, false) {
        return;
    }
    // Crop tool showing a box: O cycles its overlay, ⇧O the overlay's orientation (#1919).
    if crate::crop_overlay::keys(app, ctx) {
        return;
    }
    // ... and H toggles Show Cropped Area.
    if crate::crop_shield::keys(app, ctx) {
        return;
    }
    // Single-key tools (no modifiers). D and X (`tools.defaultColors` / `tools.swapColors`) and the
    // brush keys [ ] ⇧[ ⇧] (`tools.decreaseBrushSize`…) are commands, dispatched above with any
    // Keyboard Shortcuts override.
    let pressed = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
    // Enter / Escape commit or cancel in-progress tool state (polygonal lasso, crop).
    if crate::lasso_ui::active(app) {
        let mods = ctx.input(|i| i.modifiers);
        if ctx.input_mut(|i| i.consume_key(mods, Key::Escape)) {
            app.drag = None;
            return;
        }
        if ctx.input_mut(|i| i.consume_key(mods, Key::Enter)) {
            crate::lasso_ui::commit(app);
            return;
        }
    }
    if !app.ui.polygon.is_empty() || app.ui.crop_rect.is_some() {
        if pressed(Key::Enter) {
            if !app.ui.polygon.is_empty() {
                crate::canvas::commit_polygon(app);
            } else {
                crate::canvas::commit_crop(app);
            }
            return;
        }
        if pressed(Key::Escape) {
            app.ui.polygon.clear();
            app.ui.polygon_mode.clear();
            crate::crop_ui::cancel(app);
            return;
        }
    }
    // Escape resets a rotated canvas after the modal and in-progress gesture handlers above have
    // had first refusal. Route through the shared command so menus and automation keep the same
    // active-tab-only, view-state-only behaviour (zoom and centre are preserved).
    let rotated_view = app.session.active_index().is_some_and(|i| app.ui.views.get(i).is_some_and(|v| v.rotation != 0.0));
    if focus == Focus::None && app.drag.is_none() && rotated_view && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        if let Err(e) = crate::menus::invoke(app, ctx, "view.resetView", json!({})) {
            app.ui.status = e;
            app.ui.status_error = true;
        }
        return;
    }
    // 1–0 set opacity, ⇧1–0 flow or fill (`opacity_keys`).
    crate::opacity_keys::handle(app, ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_keys_do_not_dispatch_without_timeline_focus_owner() {
        let mut app = PhotoStoreApp::new(photostore_engine::Session::new(), Default::default());
        let ctx = egui::Context::default();
        assert!(!dispatch_timeline_shortcuts(&mut app, &ctx));
    }

    #[test]
    fn timeline_delete_is_scoped_away_from_typing_modals_and_canvas() {
        let mut app = PhotoStoreApp::new(photostore_engine::Session::new(), Default::default());
        app.run("file.new", serde_json::json!({"width":64,"height":64})).unwrap();
        app.run("timeline.create", serde_json::json!({"duration":30,"fps":30.0})).unwrap();
        let layer = app.session.active().unwrap().doc.layers[0].id.0;
        app.run("timeline.setLayerRange", serde_json::json!({"layer":layer,"start":0,"end":30})).unwrap();
        app.ui.timeline.open = true;
        let doc = app.session.active().unwrap().doc.id.0;
        let text_widget = egui::Id::new("timeline-delete-focus-test-text");
        let harness = egui_kittest::Harness::builder().with_size(egui::vec2(300.0, 200.0)).build_ui_state(
            |ui, text: &mut String| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let response = ui.add(egui::TextEdit::singleline(text).id(text_widget));
                    response.request_focus();
                });
            },
            String::from("typing"),
        );
        let ctx = harness.ctx.clone();
        ctx.data_mut(|data| data.insert_persisted(egui::Id::new("timeline-scoped-context"), doc));
        let press = |app: &mut PhotoStoreApp, ctx: &egui::Context, key| {
            let modifiers = egui::Modifiers::NONE;
            ctx.begin_pass(egui::RawInput {
                events: vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }],
                ..Default::default()
            });
            handle(app, ctx);
            let _ = ctx.end_pass();
        };

        press(&mut app, &ctx, Key::Delete);
        assert!(app.session.active().unwrap().doc.timeline.as_ref().unwrap().layer_ranges.iter().any(|range| range.layer.0 == layer));
        ctx.memory_mut(|memory| memory.surrender_focus(text_widget));
        assert_eq!(crate::shortcut_dispatch::Focus::of(&ctx), crate::shortcut_dispatch::Focus::None);
        app.ui.dialogs.push(crate::state::Dialog { id: 1, kind: crate::state::DialogKind::About, fields: Default::default() });
        press(&mut app, &ctx, Key::Delete);
        assert!(app.session.active().unwrap().doc.timeline.as_ref().unwrap().layer_ranges.iter().any(|range| range.layer.0 == layer));
        app.ui.dialogs.clear();
        ctx.data_mut(|data| data.remove::<u64>(egui::Id::new("timeline-scoped-context")));
        assert_eq!(crate::shortcut_dispatch::Focus::of(&ctx), crate::shortcut_dispatch::Focus::None);
        assert!(!dispatch_timeline_shortcuts(&mut app, &ctx), "without timeline focus owner, Delete stays in the canvas keymap");
        press(&mut app, &ctx, Key::Delete);
        assert!(app.session.active().unwrap().doc.timeline.as_ref().unwrap().layer_ranges.iter().any(|range| range.layer.0 == layer));

        let second = app.run("layer.new.layer", serde_json::json!({"name":"Second clip"})).unwrap()["layer"].as_u64().unwrap();
        app.run("timeline.setLayerRange", serde_json::json!({"layer":second,"start":0,"end":30})).unwrap();
        app.run("timeline.editor.select", serde_json::json!({"document":doc,"layers":[second]})).unwrap();
        ctx.data_mut(|data| data.insert_persisted(egui::Id::new("timeline-scoped-context"), doc));
        press(&mut app, &ctx, Key::Backspace);
        let timeline = app.session.active().unwrap().doc.timeline.as_ref().unwrap();
        assert!(timeline.layer_ranges.iter().any(|range| range.layer.0 == layer), "deleting a selected clip must leave the other clip intact");
        assert!(!timeline.layer_ranges.iter().any(|range| range.layer.0 == second), "Backspace must alias Delete in timeline context");
    }

    #[test]
    fn timeline_right_arrow_seeks_instead_of_nudging_the_active_layer() {
        let mut app = PhotoStoreApp::new(photostore_engine::Session::new(), Default::default());
        app.run("file.new", serde_json::json!({"width":64,"height":64})).unwrap();
        app.run("timeline.create", serde_json::json!({"duration":30,"fps":30.0})).unwrap();
        let layer = app.session.active().unwrap().doc.layers[0].id;
        app.run("timeline.setLayerRange", serde_json::json!({"layer":layer.0,"start":0,"end":30})).unwrap();
        app.ui.timeline.open = true;
        app.ui.tool = crate::state::Tool::Move;
        let doc = app.session.active().unwrap().doc.id.0;
        let ctx = egui::Context::default();
        ctx.data_mut(|data| data.insert_persisted(egui::Id::new("timeline-scoped-context"), doc));
        let history = app.session.active().unwrap().history.past_len();
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key { key: Key::ArrowRight, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }],
            ..Default::default()
        });
        handle(&mut app, &ctx);
        ctx.end_pass().textures_delta.clear();
        assert_eq!(app.session.active().unwrap().doc.timeline.as_ref().unwrap().current, 1);
        assert_eq!(app.session.active().unwrap().history.past_len(), history, "seeking must not commit a canvas move");
    }

    #[test]
    fn pen_command_z_and_backspace_retract_points_before_document_undo() {
        let mut app = PhotoStoreApp::new(photostore_engine::Session::new(), Default::default());
        app.run("file.new", serde_json::json!({"width": 200, "height": 200})).unwrap();
        app.ui.tool = crate::state::Tool::Pen;
        app.run("shape.create", serde_json::json!({"kind": "rect", "rect": [10, 10, 40, 40], "fill": "#ff0000"})).unwrap();
        let history = app.session.active().unwrap().history.past_len();
        for (x, y) in [(10.0, 10.0), (50.0, 10.0), (50.0, 50.0)] {
            crate::vector_ui::pen_down(&mut app, x, y);
            crate::vector_ui::pen_up(&mut app);
        }
        let ctx = egui::Context::default();
        let press = |app: &mut PhotoStoreApp, key, modifiers| {
            let raw = egui::RawInput {
                events: vec![egui::Event::ModifiersChanged(modifiers), egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }],
                ..Default::default()
            };
            ctx.begin_pass(raw);
            handle(app, &ctx);
            ctx.end_pass().textures_delta.clear();
        };

        press(&mut app, Key::Z, Modifiers::COMMAND);
        assert_eq!(app.ui.pen.as_ref().unwrap().knots.len(), 2);
        assert_eq!(app.session.active().unwrap().history.past_len(), history);
        press(&mut app, Key::Backspace, Modifiers::NONE);
        assert_eq!(app.ui.pen.as_ref().unwrap().knots.len(), 1);
        press(&mut app, Key::Delete, Modifiers::NONE);
        assert!(app.ui.pen.is_none());
        assert_eq!(app.session.active().unwrap().history.past_len(), history);
        // No unfinished Pen points remain, so regular Undo can now affect the document.
        app.run("edit.undo", serde_json::json!({})).unwrap();
        assert_eq!(app.session.active().unwrap().history.past_len(), history - 1);
    }

    #[test]
    fn pen_custom_undo_binding_and_menu_retract_pending_points() {
        let mut app = PhotoStoreApp::new(photostore_engine::Session::new(), Default::default());
        app.run("file.new", serde_json::json!({"width": 200, "height": 200})).unwrap();
        app.ui.tool = crate::state::Tool::Pen;
        for (x, y) in [(10.0, 10.0), (50.0, 10.0)] {
            crate::vector_ui::pen_down(&mut app, x, y);
            crate::vector_ui::pen_up(&mut app);
        }
        app.session.prefs.edit(|p| p.shortcuts.insert("edit.undo".into(), "Cmd+Shift+Y".into()));
        let ctx = egui::Context::default();
        let press = |app: &mut PhotoStoreApp, key: Key, modifiers: Modifiers| {
            ctx.begin_pass(egui::RawInput {
                events: vec![egui::Event::ModifiersChanged(modifiers), egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }],
                ..Default::default()
            });
            handle(app, &ctx);
            ctx.end_pass().textures_delta.clear();
        };
        // Once rebound, the default key must no longer intercept pending points.
        press(&mut app, Key::Z, Modifiers::COMMAND);
        assert_eq!(app.ui.pen.as_ref().unwrap().knots.len(), 2);
        press(&mut app, Key::Y, Modifiers::COMMAND | Modifiers::SHIFT);
        assert_eq!(app.ui.pen.as_ref().unwrap().knots.len(), 1);
        assert!(crate::menus::is_enabled(&app, "edit.undo"));
        crate::menus::invoke(&mut app, &ctx, "edit.undo", serde_json::json!({})).unwrap();
        assert!(app.ui.pen.is_none());
    }

    #[test]
    fn parses_registry_shortcuts() {
        let sc = parse("Cmd+Shift+N").unwrap();
        assert_eq!(sc.logical_key, Key::N);
        assert!(sc.modifiers.command && sc.modifiers.shift);
        assert_eq!(parse("Cmd+]").unwrap().logical_key, Key::CloseBracket);
        assert_eq!(parse("Cmd+=").unwrap().logical_key, Key::Equals);
        assert_eq!(parse("F7").unwrap().logical_key, Key::F7);
        assert!(parse("Cmd+Nonsense").is_none());
    }

    #[test]
    fn keys_match_like_photoshop() {
        let cmd = Modifiers::COMMAND;
        let zoom_in = parse("Cmd+=").unwrap();
        // ⌘= , ⌘+ typed with ⇧ (US), ⌘+ from the numpad or a Nordic/German `+` key.
        assert!(key_matches(&zoom_in, Key::Equals, cmd));
        assert!(key_matches(&zoom_in, Key::Plus, cmd | Modifiers::SHIFT));
        assert!(key_matches(&zoom_in, Key::Plus, cmd));
        assert!(!key_matches(&zoom_in, Key::Plus, Modifiers::NONE));
        assert!(!key_matches(&zoom_in, Key::Equals, cmd | Modifiers::ALT));
        assert!(key_matches(&parse("Cmd+-").unwrap(), Key::Minus, cmd));
        assert!(key_matches(&parse("Cmd+0").unwrap(), Key::Num0, cmd));
        // Ctrl (Windows) is ⌘.
        assert!(key_matches(&zoom_in, Key::Plus, Modifiers::CTRL | Modifiers { command: true, ..Default::default() }));
        // Exact modifiers: ⌘⌥I is Image Size, never Invert (⌘I); ⇧⌘Z is not ⌘Z.
        let invert = parse("Cmd+I").unwrap();
        assert!(key_matches(&invert, Key::I, cmd));
        assert!(!key_matches(&invert, Key::I, cmd | Modifiers::ALT));
        assert!(!key_matches(&parse("Cmd+Z").unwrap(), Key::Z, cmd | Modifiers::SHIFT));
        // ⇧ shortcuts on punctuation arrive as the shifted character.
        assert!(key_matches(&parse("Cmd+Shift+;").unwrap(), Key::Colon, cmd | Modifiers::SHIFT));
        assert!(key_matches(&parse("Cmd+Shift+]").unwrap(), Key::CloseCurlyBracket, cmd | Modifiers::SHIFT));
        assert!(!key_matches(&parse("Cmd+;").unwrap(), Key::Colon, cmd | Modifiers::SHIFT));
        // Delete and the Mac's delete (Backspace) both clear.
        let clear = parse("Delete").unwrap();
        assert!(key_matches(&clear, Key::Delete, Modifiers::NONE) && key_matches(&clear, Key::Backspace, Modifiers::NONE));
        assert!(!key_matches(&clear, Key::Backspace, cmd));
    }

    #[test]
    fn pretty_uses_the_platform_notation() {
        let mac = cfg!(target_os = "macos");
        assert_eq!(pretty("Cmd+Shift+N"), if mac { "⌘⇧N" } else { "Ctrl+Shift+N" });
        // Lone keys and inline-hint combos.
        assert_eq!(pretty("Alt"), if mac { "⌥" } else { "Alt" });
        assert_eq!(pretty("Shift+Alt"), if mac { "⇧⌥" } else { "Shift+Alt" });
        assert_eq!(pretty("Enter"), if mac { "↩" } else { "Enter" });
        assert_eq!(pretty("Cmd+Enter"), if mac { "⌘↩" } else { "Ctrl+Enter" });
    }

    #[test]
    fn labels_show_the_effective_shortcut_in_platform_notation() {
        let mut app = PhotoStoreApp::new(photostore_engine::Session::new(), Default::default());
        let new = command_label(&app, "New document…", "file.new");
        if cfg!(target_os = "macos") {
            assert_eq!(new, "New document…     ⌘N");
        } else {
            assert_eq!(new, "New document…     Ctrl+N");
        }
        assert_eq!(tip_label(&app, "Rulers", "view.rulers"), format!("Rulers  ({})", pretty("Cmd+R")));
        // A user override is picked up…
        app.session.prefs.edit(|p| p.shortcuts.insert("file.new".into(), "Cmd+Shift+N".into()));
        assert_eq!(command_label(&app, "New document…", "file.new"), format!("New document…     {}", pretty("Cmd+Shift+N")));
        // …and a removed binding hides the shortcut entirely.
        app.session.prefs.edit(|p| p.shortcuts.insert("file.new".into(), String::new()));
        assert_eq!(command_label(&app, "New document…", "file.new"), "New document…");
    }

    #[test]
    fn clipboard_events_become_pixel_shortcuts_outside_text() {
        let ctx = egui::Context::default();
        let raw = |mut events: Vec<egui::Event>, modifiers| {
            events.insert(0, egui::Event::ModifiersChanged(modifiers));
            egui::RawInput { events, ..Default::default() }
        };
        let keys = |r: &egui::RawInput| -> Vec<(Key, bool, bool)> {
            r.events
                .iter()
                .filter_map(|e| if let egui::Event::Key { key, pressed, modifiers, .. } = e { Some((*key, *pressed, modifiers.shift)) } else { None })
                .collect()
        };
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let mut r = raw(vec![egui::Event::Copy, egui::Event::Cut], cmd_shift);
        clipboard_keys(&ctx, false, &mut r);
        assert_eq!(keys(&r), vec![(Key::C, true, true), (Key::X, true, true)]);
        // A text paste becomes ⌘V; its key release then adds nothing.
        let up = egui::Event::Key { key: Key::V, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::COMMAND };
        let mut r = raw(vec![egui::Event::Paste("x".into()), up.clone()], Modifiers::COMMAND);
        clipboard_keys(&ctx, false, &mut r);
        assert_eq!(keys(&r), vec![(Key::V, true, false), (Key::V, false, false)]);
        // An image on the clipboard: egui-winit sends only the release. It still pastes.
        let mut r = raw(vec![up.clone()], Modifiers::COMMAND);
        clipboard_keys(&ctx, false, &mut r);
        assert_eq!(keys(&r), vec![(Key::V, true, false), (Key::V, false, false)]);
        // In a text field the events stay text clipboard events.
        let mut r = raw(vec![egui::Event::Paste("x".into())], Modifiers::COMMAND);
        clipboard_keys(&ctx, true, &mut r);
        assert!(matches!(r.events.as_slice(), [_, egui::Event::Paste(_)]));
    }

    /// #1638: every ⌘V is one press, however its key-down and key-up arrive.
    #[test]
    fn each_paste_key_is_one_press() {
        let ctx = egui::Context::default();
        let key = |key, pressed, modifiers| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
        let presses = |events: Vec<egui::Event>| {
            let mut r = egui::RawInput { events, ..Default::default() };
            clipboard_keys(&ctx, false, &mut r);
            r.events.iter().filter(|e| matches!(e, egui::Event::Key { key: Key::V, pressed: true, .. })).count()
        };
        // A ⌘V press that reached us (the Mac menu's key equivalent), then its release.
        assert_eq!(presses(vec![key(Key::V, true, Modifiers::COMMAND)]), 1);
        assert_eq!(presses(vec![key(Key::V, false, Modifiers::COMMAND)]), 0, "the release of a press doesn't paste again");
        // A text paste whose V is released after ⌘: the next image paste still pastes.
        assert_eq!(presses(vec![egui::Event::Paste("x".into())]), 1);
        assert_eq!(presses(vec![egui::Event::ModifiersChanged(Modifiers::NONE), key(Key::V, false, Modifiers::NONE)]), 0);
        assert_eq!(presses(vec![egui::Event::ModifiersChanged(Modifiers::COMMAND), key(Key::V, false, Modifiers::COMMAND)]), 1, "image paste");
    }

    /// #530: egui-winit sends Cut for Shift+Delete on Windows; on the canvas it opens Fill.
    #[test]
    fn windows_shift_delete_fills_instead_of_cutting() {
        let shift = Modifiers::SHIFT;
        let ctrl = Modifiers::CTRL | Modifiers::COMMAND;
        let app = PhotoStoreApp::new(photostore_engine::Session::new(), Default::default());
        let bound = |held| {
            let (key, mods) = clipboard_press(&egui::Event::Cut, held, true)?;
            assert_eq!((key, mods), (Key::Delete, held));
            crate::shortcut_dispatch::bindings(&app).into_iter().find(|(_, sc)| key_matches(sc, key, mods)).map(|(id, _)| id)
        };
        assert_eq!(bound(shift).as_deref(), Some("edit.fill"));
        // ⇧⌥Delete keeps ⌥: fill with the foreground, keeping transparency.
        assert_eq!(bound(shift | Modifiers::ALT).as_deref(), Some("edit.fillForegroundPreserve"));
        // Ctrl+X (and Ctrl+Shift+X) still cut; elsewhere a Cut is always ⌘X.
        assert_eq!(clipboard_press(&egui::Event::Cut, ctrl, true), Some((Key::X, ctrl)));
        assert_eq!(clipboard_press(&egui::Event::Cut, ctrl | shift, true), Some((Key::X, ctrl | shift)));
        assert_eq!(clipboard_press(&egui::Event::Cut, shift, false), Some((Key::X, Modifiers::COMMAND)));
        // Shift+Insert pastes on Windows.
        assert_eq!(clipboard_press(&egui::Event::Paste("x".into()), shift, true), Some((Key::V, Modifiers::COMMAND)));
        // Through the raw-input hook on this platform; a text field keeps its Cut.
        let ctx = egui::Context::default();
        let cut = || egui::RawInput { events: vec![egui::Event::ModifiersChanged(shift), egui::Event::Cut], ..Default::default() };
        let mut r = cut();
        clipboard_keys(&ctx, false, &mut r);
        let want = if cfg!(target_os = "windows") { Key::Delete } else { Key::X };
        assert!(matches!(r.events.as_slice(), [_, egui::Event::Key { key, pressed: true, .. }] if *key == want), "{:?}", r.events);
        let mut r = cut();
        clipboard_keys(&ctx, true, &mut r);
        assert!(matches!(r.events.as_slice(), [_, egui::Event::Cut]));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ctrl_and_shift_insert_copy_and_paste_on_linux() {
        let ctx = egui::Context::default();
        let insert = |modifiers| egui::RawInput {
            events: vec![egui::Event::Key { key: Key::Insert, physical_key: None, pressed: true, repeat: false, modifiers }],
            ..Default::default()
        };
        let pressed = |r: &egui::RawInput| match r.events.as_slice() {
            [egui::Event::Key { key, pressed: true, modifiers, .. }] if modifiers.command => Some(*key),
            _ => None,
        };
        // On the canvas they become the pixel commands' ⌘C / ⌘V.
        let mut r = insert(Modifiers::CTRL);
        clipboard_keys(&ctx, false, &mut r);
        assert_eq!(pressed(&r), Some(Key::C));
        let mut r = insert(Modifiers::SHIFT);
        clipboard_keys(&ctx, false, &mut r);
        assert_eq!(pressed(&r), Some(Key::V));
        // In a text field Ctrl+Insert copies the text; Shift+Insert asks egui-winit for the text.
        let mut r = insert(Modifiers::CTRL);
        clipboard_keys(&ctx, true, &mut r);
        assert!(matches!(r.events.as_slice(), [egui::Event::Copy]));
        let mut r = insert(Modifiers::SHIFT);
        clipboard_keys(&ctx, true, &mut r);
        assert!(r.events.is_empty());
        // Plain Insert is left alone.
        let mut r = insert(Modifiers::NONE);
        clipboard_keys(&ctx, false, &mut r);
        assert!(matches!(r.events.as_slice(), [egui::Event::Key { key: Key::Insert, .. }]));
    }

    #[test]
    fn every_registered_shortcut_parses() {
        for c in photostore_engine::command_specs() {
            if let Some(sc) = c.shortcut {
                assert!(parse(sc).is_some(), "{}: {sc}", c.id);
            }
        }
        for (id, _, _, sc) in crate::menus::UI_COMMANDS {
            if let Some(sc) = sc {
                assert!(parse(sc).is_some(), "{id}: {sc}");
            }
        }
    }

    #[test]
    fn no_duplicate_shortcuts() {
        let mut seen = std::collections::HashMap::new();
        let all = photostore_engine::command_specs()
            .iter()
            .filter_map(|c| c.shortcut.map(|s| (c.id, s)))
            .chain(crate::menus::UI_COMMANDS.iter().filter_map(|(id, _, _, s)| s.map(|s| (*id, s))));
        for (id, s) in all {
            if let Some(prev) = seen.insert(s, id) {
                panic!("shortcut {s} bound to both {prev} and {id}");
            }
        }
    }
}
