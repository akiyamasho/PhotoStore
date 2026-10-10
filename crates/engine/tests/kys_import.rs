//! `edit.keyboardShortcuts {"importKys": <xml>}`: agents, the CLI and MCP import a Photoshop
//! `.kys` set without the app's dialog (#1163).
use photocraft_engine::Session;
use photocraft_engine::prefs::normalize_shortcut;
use serde_json::json;

const SET: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<photoshop-keyboard-shortcuts version=\"4\" filename=\"$$$/FileName/Presets/KeyboardCustomization/texcuts=texcuts\">
\t<command kind=\"dynamic\" name=\"Gaussian Blur...\"><shortcut>Control+Opt+Cmd+G</shortcut></command>
\t<command kind=\"static\" name=\"Undo\" id=\"101\"><shortcut>Cmd+Z</shortcut></command>
\t<command kind=\"static\" name=\"Layer Via Copy\" id=\"2970\"><shortcut>F7</shortcut></command>
\t<command kind=\"static\" name=\"Hide Photoshop\" id=\"7001\"><shortcut>Cmd+H</shortcut></command>
\t<tool name=\"Move Tool\" type=\"1\">V</tool>
</photoshop-keyboard-shortcuts>
";

#[test]
fn import_kys_applies_the_set_and_reports_it() {
    let mut s = Session::new();
    let r = s.execute("edit.keyboardShortcuts", json!({"importKys": SET})).unwrap();
    let import = &r["import"];
    assert_eq!(import["name"], "texcuts");
    assert_eq!(import["imported"], 3);
    assert_eq!(import["unknown"], json!(["Hide Photoshop"]));
    assert_eq!(import["toolKeys"], 1);
    let prefs = s.prefs();
    assert_eq!(prefs.shortcuts.get("filter.blur.gaussianBlur").and_then(|k| normalize_shortcut(k)), normalize_shortcut("Ctrl+Alt+Cmd+G"));
    assert_eq!(prefs.shortcuts.get("layer.new.layerViaCopy").map(String::as_str), Some("F7"));
    // Undo's key is its default, so it gets no override.
    assert!(!prefs.shortcuts.contains_key("edit.undo"), "{:?}", prefs.shortcuts);
}

#[test]
fn import_kys_keys_given_in_set_win_and_bad_input_is_an_error() {
    let mut s = Session::new();
    s.execute("edit.keyboardShortcuts", json!({"importKys": SET, "set": {"layer.new.layerViaCopy": "F8"}})).unwrap();
    assert_eq!(s.prefs().shortcuts.get("layer.new.layerViaCopy").map(String::as_str), Some("F8"));
    let before = s.prefs().shortcuts.clone();
    assert!(s.execute("edit.keyboardShortcuts", json!({"importKys": "<other/>"})).is_err());
    assert!(s.execute("edit.keyboardShortcuts", json!({"importKys": 7})).is_err());
    assert_eq!(s.prefs().shortcuts, before, "a refused set changes nothing");
}

#[test]
fn optional_external_fixture_imports_without_preloading_defaults() {
    let Some(path) = std::env::var_os("PHOTOSTORE_KYS_FIXTURE") else { return };
    let text = std::fs::read_to_string(path).expect("PHOTOSTORE_KYS_FIXTURE must be readable");
    let mut s = Session::new();
    assert!(s.prefs().shortcuts.is_empty());
    assert!(s.prefs().shortcut_alternates.is_empty());
    let result = s.execute("edit.keyboardShortcuts", json!({"importKys": text})).unwrap();
    assert!(result["import"]["unknown"].is_array());
    eprintln!("kys fixture import imported={} alternates={} toolKeys={} unknown={:?} unsupported={:?} unreadable={:?}",
        result["import"]["imported"], result["import"]["alternates"], result["import"]["toolKeys"],
        result["import"]["unknown"], result["import"]["unsupported"], result["import"]["unreadable"]);
    assert_eq!(s.prefs().shortcuts.get("tools.decreaseBrushSize").map(String::as_str), Some("S"));
    assert_eq!(s.prefs().shortcuts.get("tools.increaseBrushSize").map(String::as_str), Some("D"));
    assert_eq!(s.prefs().shortcuts.get("tools.tool.brush").map(String::as_str), Some("A"));
    for id in [
        "timeline.nextFrame", "timeline.previousFrame", "timeline.toggleOnionSkin",
        "layer.smartObjects.convertToSmartObject", "layer.smartObjects.newSmartObjectViaCopy", "layer.smartObjects.editContents",
    ] {
        assert!(s.prefs().shortcuts.contains_key(id), "missing imported shortcut for {id}");
    }
    assert_eq!(s.prefs().shortcut_alternates.get("edit.copy").map(Vec::as_slice), Some(["F3".to_string()].as_slice()));
    assert_eq!(s.prefs().shortcut_alternates.get("edit.paste").map(Vec::as_slice), Some(["F4".to_string()].as_slice()));
    assert_eq!(s.prefs().shortcut_alternates.get("edit.toggleLastState").map(Vec::as_slice), Some(["F1".to_string()].as_slice()));
}
