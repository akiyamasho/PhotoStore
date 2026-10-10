//! Photoshop keyboard shortcut sets (`.kys`, Edit › Keyboard Shortcuts › Save Set): parsing and
//! matching rows to our commands (#1163). `edit.keyboardShortcuts {"importKys": <xml>}` applies a
//! set here; the app's Keyboard Shortcuts dialog fills its overrides from the same plan
//! (`photostore_ui_egui::kys_import`). Tool rows are retained and mapped by stable numeric ids;
//! localized labels are used only for dynamic command rows.
//!
//! A `.kys` is XML: `<command kind="static|dynamic" name="Gaussian Blur...">` rows, each with
//! one or more `<shortcut>Control+Opt+Cmd+G</shortcut>`, then the `<tool>` keys. Photoshop
//! names the command by its menu label alone, so rows are matched to our commands by label.
//! Where one label sits in several menus (Levels… under Image › Adjustments and Layer › New
//! Adjustment Layer; All under Select, Edit › Purge, View › Show), the command whose default is
//! one of the imported keys wins, then the one that has a default at all, then the first
//! candidate (the app lists them in Photoshop's menu order). Ours are one per command, so a
//! row's first shortcut is the one kept, unless one of its keys is already the command's
//! default (Zoom In lists `Cmd++` before `Cmd+=`). Supported tool keys and functional tool rows
//! are imported; unsupported taskspaces and tool families are reported explicitly.

use std::collections::{BTreeMap, HashMap};

use crate::prefs::normalize_shortcut;

/// A set is a few tens of kilobytes; anything far larger is not one and is not parsed.
pub const MAX_BYTES: usize = 4 << 20;

/// One `<command>` row: Photoshop's label and its shortcuts, first one primary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KysCommand {
    pub name: String,
    pub shortcuts: Vec<String>,
    /// Photoshop's stable command id.  Labels are localized and are only a fallback.
    pub id: Option<String>,
    pub kind: Option<String>,
}

/// A tool row keeps its explicit empty value: an empty key means that an existing binding is
/// intentionally removed, rather than omitted from the import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KysTool {
    pub name: String,
    pub kind: u32,
    pub id: Option<String>,
    pub shortcut: Option<String>,
}

/// The parsed set: its name, its command rows and how many tools carry a key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KysSet {
    /// The set's name as Photoshop shows it (`texcuts`), from the root's `filename` attribute,
    /// which a saved set spells `$$$/FileName/Presets/KeyboardCustomization/texcuts=texcuts`.
    pub name: Option<String>,
    pub commands: Vec<KysCommand>,
    pub tools: Vec<KysTool>,
    pub tool_keys: usize,
}

/// Parse a `.kys` file (or Photoshop's live `Keyboard Shortcuts.psp`, the same XML). Rows
/// without a name or a shortcut are skipped.
pub fn parse(text: &str) -> Result<KysSet, String> {
    if text.len() > MAX_BYTES {
        return Err("not a keyboard shortcut set (.kys): larger than 4 MiB".into());
    }
    let text = text.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse(text).map_err(|e| format!("not a keyboard shortcut set (.kys): {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "photoshop-keyboard-shortcuts" {
        return Err("not a keyboard shortcut set (.kys): no <photoshop-keyboard-shortcuts> root".into());
    }
    let name = root.attribute("filename").map(|f| f.rsplit(['=', '/']).next().unwrap_or(f).trim()).filter(|n| !n.is_empty()).map(str::to_string);
    let mut set = KysSet { name, ..Default::default() };
    for node in root.children().filter(|n| n.is_element()) {
        match node.tag_name().name() {
            "command" => {
                let Some(name) = node.attribute("name").map(str::trim).filter(|n| !n.is_empty()) else { continue };
                let shortcuts: Vec<String> = node
                    .children()
                    .filter(|n| n.is_element() && n.tag_name().name() == "shortcut")
                    .filter_map(|n| n.text().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string))
                    .collect();
                set.commands.push(KysCommand {
                    name: name.to_string(),
                    shortcuts,
                    id: node.attribute("id").map(str::to_string),
                    kind: node.attribute("kind").map(str::to_string),
                });
            }
            "tool" => {
                let kind = node.attribute("type").and_then(|s| s.parse().ok()).unwrap_or(0);
                let shortcut = node.text().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
                if shortcut.is_some() { set.tool_keys += 1; }
                set.tools.push(KysTool {
                    name: node.attribute("name").unwrap_or_default().to_string(),
                    kind,
                    id: node.attribute("key").map(str::to_string),
                    shortcut,
                });
            }
            _ => {}
        }
    }
    Ok(set)
}

/// What an import would change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Command id → the normalised shortcut from the file, for every matched row.
    pub set: BTreeMap<String, String>,
    /// Additional normalized bindings per command, retaining every supported Photoshop key.
    pub alternates_for: BTreeMap<String, Vec<String>>,
    /// Photoshop names with no command here (scripts, Bridge, the macOS application menu…).
    pub unknown: Vec<String>,
    /// Rows whose shortcut could not be read (`unknown` does not list them).
    pub unreadable: Vec<String>,
    /// Second and later shortcuts of a row, which have no place here.
    pub alternates: usize,
    /// Tool and command rows whose stable id is known to Photoshop but unsupported here.
    pub unsupported: Vec<String>,
}

/// One command a row may match: its id, its label and its default shortcut.
pub struct Candidate {
    pub id: String,
    pub label: String,
    pub default: Option<String>,
}

/// Stable Photoshop ids are preferred over localized labels.  Keep this table small and
/// explicit: silently guessing an unrelated command is worse than reporting an unsupported row.
pub fn stable_command_id(id: &str) -> Option<&'static str> {
    Some(match id {
        "10" => "file.new", "20" => "file.open", "30" => "file.save", "31" => "file.close", "33" => "file.saveACopy", "34" => "file.revert", "36" => "file.exit", "37" => "file.closeAll",
        "101" => "edit.undo", "103" => "edit.cut", "104" => "edit.copy", "105" => "edit.paste", "132" => "edit.redo", "133" => "edit.toggleLastState",
        "177" => "file.print", "1004" => "view.zoomIn", "1005" => "view.zoomOut", "1007" => "image.mode.grayscale", "1016" => "select.deselect", "1017" => "select.all", "1018" => "select.inverse", "1019" => "filter.lastFilter", "1025" => "window.panel.brushSettings", "1030" => "image.imageSize", "1036" => "select.modify.feather", "1042" => "edit.fill", "1044" => "image.crop", "1046" => "window.panel.color", "1098" => "window.panel.layers", "1099" => "layer.new.layer", "1105" => "view.proofColors", "1107" => "edit.copyMerged", "1109" => "select.modify.expand", "1114" => "layer.hideLayers", "1135" => "layer.duplicate", "1137" => "file.fileInfo", "1152" => "select.colorRange", "1166" => "layer.mergeLayers", "1170" => "window.panel.actions", "1188" => "window.panel.timeline", "1190" => "view.actualPixels", "1192" => "view.fitOnScreen", "2962" => "select.allLayers", "2982" => "select.findLayers",
        "1297" => "edit.pasteSpecial.pasteInPlace", "1314" => "layer.layerStyle.copyLayerStyle", "1315" => "layer.layerStyle.pasteLayerStyle", "1339" => "layer.layerStyle.clear", "1695" => "file.export.saveForWebLegacy", "1701" => "image.adjustments.invert", "1801" => "image.adjustments.levels", "1802" => "image.adjustments.curves", "1804" => "image.adjustments.colorBalance", "1805" => "image.adjustments.hueSaturation", "1808" => "image.autoTone", "1809" => "image.adjustments.desaturate", "1810" => "image.autoContrast", "1817" => "image.autoColor", "1824" => "image.adjustments.blackWhite",
        "2101" => "file.print", "2202" => "filter.liquify", "2207" => "edit.freeTransform", "2211" => "edit.transform.flipHorizontal", "2212" => "edit.transform.flipVertical", "2711" => "layer.arrange.bringToFront", "2712" => "layer.arrange.bringForward", "2713" => "layer.arrange.sendBackward", "2714" => "layer.arrange.sendToBack",
        "2219" => "edit.transform.warp", "2221" => "edit.puppetWarp", "2311" => "edit.preferences.general", "2715" => "layer.arrange.reverse", "2860" => "image.analysis.recordMeasurements", "2940" => "view.lockGuides", "2957" => "layer.lockLayers", "2958" => "layer.groupLayers", "2970" => "layer.new.layerViaCopy", "2971" => "layer.new.layerViaCut", "2976" => "layer.new.group", "2980" => "layer.smartObjects.convertToSmartObject", "2981" => "layer.smartObjects.newSmartObjectViaCopy", "2983" => "layer.renameLayer", "3090" => "file.placeLinked", "3444" => "file.export.quickExportAsPng", "3446" => "layer.quickExportAsPng", "3447" => "layer.exportAs", "3500" => "view.extras", "3502" => "view.show.targetPath", "3503" => "view.show.guides", "3504" => "view.show.grid", "3520" => "view.snap", "3547" => "view.show.layerEdges",
        "4418" => "timeline.nextFrame",
        "4419" => "timeline.previousFrame",
        "4412" => "timeline.toggleOnionSkin",
        "4462" => "timeline.deleteFrame", "4469" => "timeline.makeFramesFromLayers", "5957" => "edit.search", "5980" => "edit.keyboardShortcuts", "5982" => "edit.menus", "6332" => "layer.smartObjects.editContents", "6340" => "layer.smartObjects.relinkToFile", "6342" => "layer.smartObjects.updateModifiedContent",
        _ => return None,
    })
}

pub fn tool_command_id(kind: u32, key_id: Option<&str>) -> Option<&'static str> {
    match (kind, key_id) {
        (1, Some("1819113074")) => Some("tools.tool.move"),
        (1, Some("1919380852")) => Some("tools.tool.rectMarquee"), (1, Some("1701604724")) => Some("tools.tool.ellipseMarquee"),
        (1, Some("1818325871")) => Some("tools.tool.lasso"), (1, Some("1885826926")) => Some("tools.tool.polygonLasso"), (1, Some("1835819379")) => Some("tools.tool.magneticLasso"),
        (1, Some("2002873956")) => Some("tools.tool.magicWand"),
        (1, Some("1902867308")) => Some("tools.tool.quickSelection"), (1, Some("1835494497")) => Some("tools.tool.objectSelection"),
        (1, Some("1886286946")) => Some("tools.tool.brush"),
        (1, Some("1885695587")) => Some("tools.tool.pencil"),
        (1, Some("1701994867")) => Some("tools.tool.eraser"), (1, Some("1936028257")) => Some("tools.tool.backgroundEraser"), (1, Some("1835364961")) => Some("tools.tool.magicEraser"),
        (1, Some("1651401812")) => Some("tools.tool.gradient"),
        (1, Some("1651860331")) => Some("tools.tool.paintBucket"),
        (1, Some("1954038392")) => Some("tools.tool.type"), (1, Some("1954038358")) => Some("tools.tool.verticalType"),
        (1, Some("1751215716")) => Some("tools.tool.hand"),
        (1, Some("1919906932")) => Some("tools.tool.rotateView"),
        (1, Some("2054123373")) => Some("tools.tool.zoom"),
        (1, Some("1668444016")) => Some("tools.tool.crop"), (1, Some("1399612244")) => Some("tools.tool.slice"), (1, Some("1399608148")) => Some("tools.tool.sliceSelect"),
        (1, Some("1702454628")) => Some("tools.tool.eyedropper"), (1, Some("1835360596")) => Some("tools.tool.ruler"), (1, Some("1417180225")) => Some("tools.tool.note"), (1, Some("1668248942")) => Some("tools.tool.count"),
        (1, Some("1667327604")) => Some("tools.tool.remove"), (1, Some("1936746562")) => Some("tools.tool.spotHealing"), (1, Some("1937010029")) => Some("tools.tool.healing"), (1, Some("1886675816")) => Some("tools.tool.patch"), (1, Some("1919118704")) => Some("tools.tool.contentAwareMove"), (1, Some("1919182201")) => Some("tools.tool.redEye"),
        (1, Some("1937006957")) => Some("tools.tool.cloneStamp"), (1, Some("1937010032")) => Some("tools.tool.patternStamp"), (1, Some("1752396866")) => Some("tools.tool.historyBrush"), (1, Some("2003137634")) => Some("tools.tool.mixerBrush"),
        (1, Some("1651275122")) => Some("tools.tool.blur"),
        (1, Some("1936220530")) => Some("tools.tool.sharpen"),
        (1, Some("1936553316")) => Some("tools.tool.smudge"),
        (1, Some("1685021799")) => Some("tools.tool.dodge"), (1, Some("1651864174")) => Some("tools.tool.burn"), (1, Some("1935766645")) => Some("tools.tool.sponge"),
        (1, Some("1885695572")) | (1, Some("1836082542")) | (1, Some("1668310382")) => Some("tools.tool.pen"),
        (1, Some("1885565780")) => Some("tools.tool.directSelection"), (1, Some("1886677089")) => Some("tools.tool.pathSelection"),
        (1, Some("1382376308")) => Some("tools.tool.rectangle"), (1, Some("1164734579")) => Some("tools.tool.ellipseShape"), (1, Some("1416778600")) => Some("tools.tool.triangle"), (1, Some("1349479545")) => Some("tools.tool.polygon"), (1, Some("1818848852")) => Some("tools.tool.line"), (1, Some("1131762536")) => Some("tools.tool.customShape"),
        (7, _) => Some("tools.decreaseBrushSize"),
        (8, _) => Some("tools.increaseBrushSize"),
        (9, _) => Some("tools.decreaseBrushHardness"),
        (10, _) => Some("tools.increaseBrushHardness"),
        (11, _) => Some("tools.previousBrush"),
        (12, _) => Some("tools.nextBrush"),
        (13, _) => Some("tools.firstBrush"),
        (14, _) => Some("tools.lastBrush"),
        (2, _) => Some("tools.defaultColors"),
        (3, _) => Some("tools.swapColors"),
        (4, _) => Some("tools.toggleQuickMask"),
        (5, _) => Some("tools.toggleScreenMode"),
        (6, _) => Some("tools.toggleTransparentPixels"),
        (15, _) => Some("tools.colorPickerForeground"),
        (16, _) => Some("tools.colorPickerBackground"),
        (29, _) => Some("tools.togglePreviewMode"),
        _ => None,
    }
}

/// Photoshop's label and ours compared loosely: case, a trailing ellipsis (`...` or `…`) and
/// surrounding space do not count.
fn key(label: &str) -> String {
    let l = label.trim().replace("...", "…");
    l.trim_end_matches('…').trim().to_lowercase()
}

/// The engine's own candidates, including no-menu timeline and smart-object commands. Stable
/// Photoshop ids can therefore resolve commands that are absent from the visible menu.
pub fn command_candidates() -> Vec<Candidate> {
    let mut out: Vec<Candidate> = crate::command_specs()
        .iter()
        .map(|c| Candidate { id: c.id.to_string(), label: c.label.to_string(), default: c.shortcut.map(str::to_string) })
        .collect();
    out.extend([("tools.previousBrush", "Previous Brush"), ("tools.nextBrush", "Next Brush"), ("tools.firstBrush", "First Brush"), ("tools.lastBrush", "Last Brush")].into_iter().map(|(id, label)| Candidate { id: id.into(), label: label.into(), default: None }));
    out.push(Candidate { id: "edit.freeTransform".into(), label: "Free Transform".into(), default: Some("Cmd+T".into()) });
    for id in [
        "tools.tool.move", "tools.tool.rectMarquee", "tools.tool.ellipseMarquee", "tools.tool.lasso", "tools.tool.polygonLasso", "tools.tool.magneticLasso", "tools.tool.magicWand", "tools.tool.quickSelection", "tools.tool.objectSelection", "tools.tool.brush", "tools.tool.pencil", "tools.tool.mixerBrush", "tools.tool.historyBrush", "tools.tool.eraser", "tools.tool.backgroundEraser", "tools.tool.magicEraser", "tools.tool.gradient", "tools.tool.paintBucket", "tools.tool.type", "tools.tool.verticalType", "tools.tool.hand", "tools.tool.rotateView", "tools.tool.zoom", "tools.tool.eyedropper", "tools.tool.ruler", "tools.tool.note", "tools.tool.count", "tools.tool.crop", "tools.tool.slice", "tools.tool.sliceSelect", "tools.tool.remove", "tools.tool.spotHealing", "tools.tool.healing", "tools.tool.patch", "tools.tool.contentAwareMove", "tools.tool.redEye", "tools.tool.cloneStamp", "tools.tool.patternStamp", "tools.tool.blur", "tools.tool.sharpen", "tools.tool.smudge", "tools.tool.dodge", "tools.tool.burn", "tools.tool.sponge", "tools.tool.pen", "tools.tool.pathSelection", "tools.tool.directSelection", "tools.tool.rectangle", "tools.tool.ellipseShape", "tools.tool.triangle", "tools.tool.polygon", "tools.tool.line", "tools.tool.customShape", "tools.decreaseBrushSize", "tools.increaseBrushSize", "tools.decreaseBrushHardness", "tools.increaseBrushHardness", "tools.refillMixerBrush", "tools.cleanMixerBrush", "tools.toggleMixerAutoFill", "tools.toggleMixerAutoClean", "tools.toggleMixerSampleAllLayers", "tools.sharpenErodibleTip", "tools.togglePathSelectionMode", "tools.toggleAirbrushMode", "tools.togglePressureSize", "tools.togglePressureOpacity", "tools.toggleSymmetryOffLast", "tools.toggleSymmetry", "tools.togglePreviewMode", "tools.brushSwitchErase"
    ] {
        if !out.iter().any(|c| c.id == id) { out.push(Candidate { id: id.into(), label: id.into(), default: None }); }
    }
    out
}

/// Match the rows to `candidates` (see the module notes for the tie-breaks). One command may be
/// listed under several labels (the menu's Settings… is the command's General…); under each
/// label it counts once.
pub fn plan(candidates: impl IntoIterator<Item = Candidate>, set: &KysSet) -> Plan {
    let candidates: Vec<Candidate> = candidates.into_iter().collect();
    let mut by_label: HashMap<String, Vec<(String, Option<String>)>> = HashMap::new();
    for c in &candidates {
        let entry = by_label.entry(key(&c.label)).or_default();
        if !entry.iter().any(|(id, _)| *id == c.id) {
            entry.push((c.id.clone(), c.default.clone()));
        }
    }
    let mut plan = Plan::default();
    for row in &set.commands {
        let stable = row.id.as_deref().and_then(stable_command_id);
        let stable_candidate = stable.and_then(|id| candidates.iter().find(|c| c.id == id));
        if stable.is_some() && stable_candidate.is_none() {
            plan.unsupported.push(row.name.clone());
            continue;
        }
        let Some(candidates) = stable_candidate.map(|c| vec![(c.id.clone(), c.default.clone())]).or_else(|| by_label.get(&key(&row.name)).cloned()) else {
            plan.unknown.push(row.name.clone());
            continue;
        };
        if row.shortcuts.is_empty() {
            plan.set.insert(stable_candidate.map(|c| c.id.clone()).or_else(|| candidates.first().map(|c| c.0.clone())).unwrap_or_default(), String::new());
            continue;
        }
        let Some(sc) = row.shortcuts.first().and_then(|s| normalize_shortcut(s)) else {
            plan.unreadable.push(row.name.clone());
            continue;
        };
        let imported: Vec<String> = row.shortcuts.iter().filter_map(|s| normalize_shortcut(s)).collect();
        plan.alternates += imported.len().saturating_sub(1);
        let pick = candidates
            .iter()
            .find(|(_, d)| d.as_deref().and_then(normalize_shortcut).is_some_and(|d| imported.contains(&d)))
            .or_else(|| candidates.iter().find(|(_, d)| d.is_some()))
            .or_else(|| candidates.first());
        if let Some((id, default)) = pick {
            // A row that lists the command's own default among its keys keeps the default
            // (Zoom In is "Cmd++" then "Cmd+=" in Photoshop; ours is Cmd+=).
            let default = default.as_deref().and_then(normalize_shortcut);
            let sc = default.filter(|d| imported.contains(d)).unwrap_or(sc);
            plan.set.insert(id.clone(), sc.clone());
            let alts: Vec<String> = imported.into_iter().filter(|k| k != &sc).collect();
            if !alts.is_empty() { plan.alternates_for.insert(id.clone(), alts); }
        }
    }
    for tool in &set.tools {
        let Some(id) = tool_command_id(tool.kind, tool.id.as_deref()) else {
            let taskspace = tool.name.to_ascii_lowercase().contains("3d") || tool.name.to_ascii_lowercase().contains("artboard") || tool.name.to_ascii_lowercase().contains("material");
            if !taskspace && tool.id.is_some() { plan.unsupported.push(tool.name.clone()); }
            continue;
        };
        let Some(sc) = tool.shortcut.as_deref().and_then(normalize_shortcut) else {
            // Empty tool entries are explicit clears when the tool has a known command id.
            if tool.shortcut.is_none() {
                plan.set.insert(id.to_string(), String::new());
            }
            continue;
        };
        if candidates.iter().any(|c| c.id == id) {
            plan.set.insert(id.to_string(), sc);
        } else {
            plan.unsupported.push(tool.name.clone());
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    const SET: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<photoshop-keyboard-shortcuts version=\"4\" filename=\"$$$/FileName/Presets/KeyboardCustomization/texcuts=texcuts\">
\t<command kind=\"static\" name=\"Levels...\"><shortcut>Cmd+L</shortcut></command>
\t<command kind=\"static\" name=\"Undo\" id=\"101\"><shortcut>Cmd+Z</shortcut><shortcut>F1</shortcut></command>
\t<command kind=\"static\" name=\"Hide Photoshop\" id=\"7001\"><shortcut>Cmd+H</shortcut></command>
\t<command kind=\"static\" name=\"Odd\" id=\"1\"><shortcut>Cmd+Shift+Ctrl</shortcut></command>
\t<tool name=\"Move Tool\" type=\"1\">V</tool>
</photoshop-keyboard-shortcuts>
";

    fn c(id: &str, label: &str, default: Option<&str>) -> Candidate {
        Candidate { id: id.into(), label: label.into(), default: default.map(str::to_string) }
    }

    #[test]
    fn rows_match_by_label_and_the_default_holder_wins_a_shared_label() {
        let set = parse(SET).unwrap();
        assert_eq!(set.name.as_deref(), Some("texcuts"));
        assert_eq!(set.tool_keys, 1);
        let p = plan(
            [c("layer.newLevels", "Levels…", None), c("image.levels", "Levels…", Some("Cmd+L")), c("edit.undo", "Undo", Some("Cmd+Z")), c("odd", "Odd", None)],
            &set,
        );
        assert_eq!(p.set.get("image.levels").map(String::as_str), Some("Cmd+L"));
        assert!(!p.set.contains_key("layer.newLevels"));
        assert_eq!(p.unknown, ["Hide Photoshop"]);
        assert_eq!(p.unreadable, ["Odd"]);
        assert_eq!(p.alternates, 1);
        assert!(p.unsupported.is_empty());
    }

    #[test]
    fn oversized_input_is_refused_before_parsing() {
        let big = format!("<photoshop-keyboard-shortcuts>{}</photoshop-keyboard-shortcuts>", " ".repeat(MAX_BYTES));
        assert!(parse(&big).unwrap_err().contains("4 MiB"));
        assert!(parse("<other/>").unwrap_err().contains("not a keyboard shortcut set"));
    }

    #[test]
    fn localized_stable_ids_and_explicit_tool_clears_survive() {
        let set = parse(r#"<photoshop-keyboard-shortcuts>
          <command kind="static" id="2980" name="スマートオブジェクトに変換"><shortcut>Cmd+Shift+S</shortcut></command>
          <command kind="static" id="4418" name="次のフレーム"><shortcut>F2</shortcut></command>
          <tool name="ブラシツール" type="1" key="1886286946">A</tool>
          <tool name="ブラシサイズを減少" type="7">S</tool>
          <tool name="アートボードツール" type="1" key="1098019924"></tool>
        </photoshop-keyboard-shortcuts>"#).unwrap();
        assert_eq!(set.tools.len(), 3);
        let p = plan(
            [
                c("layer.smartObjects.convertToSmartObject", "Convert", None),
                c("timeline.nextFrame", "Next Frame", None),
                c("tools.tool.brush", "Brush Tool", Some("B")),
                c("tools.decreaseBrushSize", "Decrease Brush Size", Some("[")),
            ],
            &set,
        );
        assert_eq!(p.set.get("layer.smartObjects.convertToSmartObject"), Some(&"Cmd+Shift+S".to_string()));
        assert_eq!(p.set.get("timeline.nextFrame"), Some(&"F2".to_string()));
        assert_eq!(p.set.get("tools.tool.brush"), Some(&"A".to_string()));
        assert_eq!(p.set.get("tools.decreaseBrushSize"), Some(&"S".to_string()));
    }

    #[test]
    fn supplied_fixture_keeps_alternates_and_reports_only_real_unmapped_rows() {
        let Some(path) = std::env::var_os("PHOTOSTORE_KYS_FIXTURE") else { return };
        let text = std::fs::read_to_string(path).expect("PHOTOSTORE_KYS_FIXTURE must be readable");
        let set = parse(&text).unwrap();
        let plan = plan(command_candidates(), &set);
        assert_eq!(plan.set.get("edit.copy").map(String::as_str), Some("Cmd+C"));
        assert_eq!(plan.set.get("edit.paste").map(String::as_str), Some("Cmd+V"));
        assert_eq!(plan.set.get("edit.toggleLastState").map(String::as_str), Some("Cmd+Alt+Z"));
        assert_eq!(plan.alternates_for.get("edit.copy").map(Vec::as_slice), Some(["F3".to_string()].as_slice()));
        assert_eq!(plan.alternates_for.get("edit.paste").map(Vec::as_slice), Some(["F4".to_string()].as_slice()));
        assert_eq!(plan.alternates_for.get("edit.toggleLastState").map(Vec::as_slice), Some(["F1".to_string()].as_slice()));
        assert!(plan.set.contains_key("image.crop"));
        assert!(plan.set.contains_key("edit.freeTransform"));
        assert!(!plan.set.contains_key("tools.tool.perspectiveCrop"));
        assert_eq!(plan.set.get("tools.decreaseBrushSize").map(String::as_str), Some("S"));
        assert_eq!(plan.set.get("tools.increaseBrushSize").map(String::as_str), Some("D"));
        assert_eq!(plan.set.get("tools.tool.cloneStamp").map(String::as_str), Some(""));
        assert_eq!(plan.set.get("tools.tool.historyBrush").map(String::as_str), Some(""));
        assert_eq!(plan.set.get("tools.tool.mixerBrush").map(String::as_str), Some(""));
        assert!(plan.set.contains_key("timeline.nextFrame"));
        assert!(plan.set.contains_key("timeline.previousFrame"));
        assert!(plan.set.contains_key("timeline.toggleOnionSkin"));
        assert!(plan.set.contains_key("layer.smartObjects.convertToSmartObject"));
        assert!(plan.set.contains_key("layer.smartObjects.newSmartObjectViaCopy"));
        assert!(plan.set.contains_key("layer.smartObjects.editContents"));
        eprintln!("kys fixture unknown={:?} unsupported={:?}", plan.unknown, plan.unsupported);
    }
}
