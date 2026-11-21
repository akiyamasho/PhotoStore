//! A notebook page is a root layer in Layers-panel order. Navigation only changes the
//! session's edit target. The render path projects that root from the canonical document.
use crate::{DocState, EngineError, Result, Session};
use photostore_doc::{Layer, LayerContent, LayerId};
use serde_json::{Value, json};

fn contains(layer: &Layer, id: LayerId) -> bool {
    layer.id == id || layer.children().is_some_and(|children| children.iter().any(|child| contains(child, id)))
}

impl DocState {
    pub fn notebook_root(&self) -> Option<LayerId> {
        if !self.notebook {
            return None;
        }
        let roots = &self.doc.layers;
        self.active_layer.and_then(|id| roots.iter().rev().find(|root| contains(root, id))).or_else(|| roots.last()).map(|root| root.id)
    }
}

pub(crate) fn enabled(s: &Session) -> std::result::Result<(), String> {
    s.active().filter(|st| st.notebook).map(|_| ()).ok_or_else(|| "Open Notebook View first".into())
}

fn bad(cmd: &str, msg: &str) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

fn root_id(cmd: &str, p: &Value) -> Result<LayerId> {
    p.get("layer").and_then(Value::as_u64).map(LayerId).ok_or_else(|| bad(cmd, "Expected a top-level layer ID"))
}

/// Choose a drawable child on a newly opened group page, respecting inherited locks and visibility.
fn drawable(layer: &Layer) -> Option<LayerId> {
    if !layer.visible || layer.locks.all || layer.locks.pixels {
        return None;
    }
    if matches!(layer.content, LayerContent::Raster(_)) {
        return Some(layer.id);
    }
    layer.children()?.iter().rev().find_map(drawable)
}

fn open_page(s: &mut Session, root_id: LayerId) -> Result<Value> {
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let root = st.doc.layers.iter().find(|root| root.id == root_id).ok_or_else(|| bad("notebook.select", "Page must be a top-level layer or group"))?;
    let target = st.active_layer.filter(|&id| id != root_id && contains(root, id)).or_else(|| drawable(root)).unwrap_or(root_id);
    s.select_layer(target)?;
    Ok(json!({"page": root_id.0, "layer": target.0}))
}

pub(crate) fn toggle(s: &mut Session, p: &Value) -> Result<Value> {
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    let wanted = match p.get("enabled") {
        Some(value) => value.as_bool().ok_or_else(|| bad("view.notebook", "enabled must be a boolean"))?,
        None => !st.notebook,
    };
    if st.notebook != wanted {
        st.notebook = wanted;
        if let Some(root) = st.notebook_root() {
            open_page(s, root)?;
        }
    }
    Ok(json!({"enabled": wanted}))
}

pub(crate) fn select(s: &mut Session, p: &Value) -> Result<Value> {
    open_page(s, root_id("notebook.select", p)?)
}

pub(crate) fn step(s: &mut Session, delta: isize) -> Result<Value> {
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let roots = &st.doc.layers;
    let Some(index) = st.notebook_root().and_then(|id| roots.iter().position(|root| root.id == id)) else {
        return Ok(Value::Null);
    };
    let Some(next) = index.checked_add_signed(delta).and_then(|i| roots.get(i)) else {
        return Ok(json!({"page": roots[index].id.0}));
    };
    open_page(s, next.id)
}

pub(crate) fn move_page(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "notebook.move";
    let page = root_id(cmd, p)?;
    let (delta, placement) = match p.get("direction").and_then(Value::as_str) {
        Some("up") => (1, "above"),
        Some("down") => (-1, "below"),
        _ => return Err(bad(cmd, "direction must be up or down")),
    };
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let roots = &st.doc.layers;
    let index = roots.iter().position(|root| root.id == page).ok_or_else(|| bad(cmd, "Page must be a top-level layer or group"))?;
    let Some(neighbour) = index.checked_add_signed(delta).and_then(|i| roots.get(i)) else {
        return Ok(json!({"moved": false}));
    };
    let edit_target = st.active_layer;
    let params = json!({"layer": page.0, "target": neighbour.id.0, "position": placement});
    crate::layer_multi_cmds::move_to(s, &params)?;
    if let Some(target) = edit_target {
        s.select_layer(target)?;
    }
    Ok(json!({"moved": true}))
}

/// Create a view-only snapshot containing a single root layer and its complete subtree.
/// The source document and its visibility, history, and serialized data are untouched.
pub fn project(doc: &photostore_doc::Document, root: photostore_doc::LayerId) -> photostore_doc::Document {
    let mut projected = doc.clone();
    projected.layers.retain(|layer| layer.id == root);
    projected
}

