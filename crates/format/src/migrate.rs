//! Manifest migrations. Each step upgrades the raw JSON from version `n` to
//! `n + 1`, so old bundles keep loading as the schema evolves.

use serde_json::Value;

use crate::manifest::FORMAT_VERSION;
use crate::{FormatError, Result};

pub(crate) type Step = fn(&mut Value) -> Result<()>;

fn v1_to_v2(_v: &mut Value) -> Result<()> {
    Ok(())
}
fn v2_to_v3(v: &mut Value) -> Result<()> {
    let document = v.get_mut("document").and_then(Value::as_object_mut).ok_or_else(|| FormatError::corrupt("manifest has no document object"))?;
    document.entry("media_assets").or_insert_with(|| Value::Array(Vec::new()));
    let mut next_id = document.get("id").and_then(Value::as_u64).unwrap_or(0);
    fn reserve_layer_ids(layers: &[Value], max: &mut u64) {
        for layer in layers {
            *max = (*max).max(layer.get("id").and_then(Value::as_u64).unwrap_or(0));
            if let Some(content) = layer.get("content") {
                *max = (*max).max(content.get("contents_id").and_then(Value::as_u64).unwrap_or(0));
                if let Some(children) = content.get("children").and_then(Value::as_array) { reserve_layer_ids(children, max); }
            }
        }
    }
    if let Some(layers) = document.get("layers").and_then(Value::as_array) { reserve_layer_ids(layers, &mut next_id); }
    let mut ids_by_path = std::collections::HashMap::<String, u64>::new();
    let mut assets = Vec::<Value>::new();
    fn bind_path(path: &str, id_by_path: &mut std::collections::HashMap<String, u64>, assets: &mut Vec<Value>, next_id: &mut u64) -> Result<Option<u64>> {
        if path.is_empty() { return Ok(None); }
        if let Some(id) = id_by_path.get(path) { return Ok(Some(*id)); }
        *next_id = next_id.checked_add(1).ok_or_else(|| FormatError::corrupt("media source identity space exhausted during migration"))?;
        let id = *next_id;
        id_by_path.insert(path.to_owned(), id);
        assets.push(serde_json::json!({"id":id,"revision":1,"path":path,"fingerprint":null,"metadata":null}));
        Ok(Some(id))
    }
    fn path_at<'a>(value: &'a Value, variant: &str) -> Option<&'a str> {
        value.get(variant).and_then(|payload| payload.get("path")).and_then(Value::as_str)
    }
    fn bind_layers(
        layers: &mut [Value],
        ids: &mut std::collections::HashMap<String, u64>,
        assets: &mut Vec<Value>,
        next_id: &mut u64,
    ) -> Result<()> {
        for layer in layers {
            if let Some(video) = layer.get_mut("video").and_then(Value::as_object_mut) {
                let path = video.get("source").and_then(|source| path_at(source, "File")).map(str::to_owned);
                video.insert("asset_id".into(), match path {
                    Some(path) => bind_path(&path, ids, assets, next_id)?.map_or(Value::Null, |id| Value::from(id)),
                    None => Value::Null,
                });
            }
            if let Some(content) = layer.get_mut("content").and_then(Value::as_object_mut) {
                if content.get("kind").and_then(Value::as_str) == Some("smart") {
                    let path = content.get("source").filter(|source| source.get("kind").and_then(Value::as_str) == Some("linked"))
                        .and_then(|source| source.get("path")).and_then(Value::as_str).map(str::to_owned);
                    content.insert("asset_id".into(), match path {
                        Some(path) => bind_path(&path, ids, assets, next_id)?.map_or(Value::Null, |id| Value::from(id)),
                        None => Value::Null,
                    });
                }
                if let Some(children) = content.get_mut("children").and_then(Value::as_array_mut) {
                    bind_layers(children, ids, assets, next_id)?;
                }
            }
        }
        Ok(())
    }
    if let Some(layers) = document.get_mut("layers").and_then(Value::as_array_mut) {
        bind_layers(layers, &mut ids_by_path, &mut assets, &mut next_id)?;
    }
    if let Some(timeline) = document.get_mut("timeline").and_then(Value::as_object_mut) {
        if let Some(tracks) = timeline.get_mut("audio_tracks").and_then(Value::as_array_mut) {
            for clip in tracks.iter_mut().flat_map(|track| track.get_mut("clips").and_then(Value::as_array_mut).into_iter().flatten()) {
                let path = clip.get("path").and_then(Value::as_str).map(str::to_owned);
                clip.as_object_mut().ok_or_else(|| FormatError::corrupt("audio clip is not an object"))?.insert(
                    "asset_id".into(),
                    match path {
                        Some(path) => bind_path(&path, &mut ids_by_path, &mut assets, &mut next_id)?.map_or(Value::Null, |id| Value::from(id)),
                        None => Value::Null,
                    },
                );
            }
        }
    }
    document.insert("media_assets".into(), Value::Array(assets));
    Ok(())
}
/// `STEPS[i]` upgrades version `i + 1` to `i + 2`.
pub(crate) const STEPS: &[Step] = &[v1_to_v2, v2_to_v3];

/// Upgrade `v` in place to `target` using `steps`.
pub(crate) fn migrate_with(v: &mut Value, steps: &[Step], target: u32) -> Result<u32> {
    let found = v.get("format_version").and_then(Value::as_u64).ok_or_else(|| FormatError::corrupt("manifest has no format_version"))?;
    let found = u32::try_from(found).map_err(|_| FormatError::corrupt("format_version out of range"))?;
    if found == 0 {
        return Err(FormatError::corrupt("format_version 0 is invalid"));
    }
    if found > target {
        return Err(FormatError::TooNew { found, supported: target });
    }
    for ver in found..target {
        let step = steps.get(ver as usize - 1).ok_or_else(|| FormatError::Unsupported(format!("no migration from format {ver}")))?;
        step(v)?;
        v["format_version"] = Value::from(ver + 1);
    }
    Ok(found)
}

pub(crate) fn migrate(v: &mut Value) -> Result<u32> {
    migrate_with(v, STEPS, FORMAT_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rename_title(v: &mut Value) -> Result<()> {
        let t = v["document"]["title"].take();
        v["document"]["name"] = t;
        Ok(())
    }
    fn add_field(v: &mut Value) -> Result<()> {
        v["document"]["added"] = json!(true);
        Ok(())
    }

    #[test]
    fn chained_steps_run_in_order() {
        let mut v = json!({"format_version": 1, "document": {"title": "old"}});
        let found = migrate_with(&mut v, &[rename_title, add_field], 3).unwrap();
        assert_eq!(found, 1);
        assert_eq!(v["format_version"], 3);
        assert_eq!(v["document"]["name"], "old");
        assert_eq!(v["document"]["added"], true);
    }

    #[test]
    fn partial_chain_from_middle() {
        let mut v = json!({"format_version": 2, "document": {}});
        migrate_with(&mut v, &[rename_title, add_field], 3).unwrap();
        assert_eq!(v["document"]["added"], true);
        assert!(v["document"].get("name").is_none());
    }

    #[test]
    fn too_new_and_invalid_versions() {
        let mut v = json!({"format_version": 99});
        assert!(matches!(migrate(&mut v), Err(FormatError::TooNew { found: 99, .. })));
        assert!(migrate(&mut json!({"format_version": 0})).is_err());
        assert!(migrate(&mut json!({})).is_err());
        assert!(migrate_with(&mut json!({"format_version": 1}), &[], 2).is_err());
    }

    #[test]
    fn current_version_is_noop() {
        let mut v = json!({"format_version": FORMAT_VERSION});
        assert_eq!(migrate(&mut v).unwrap(), FORMAT_VERSION);
    }

    #[test]
    fn timeline_completion_data_boundary_v2_to_v3_binds_shared_sources_once() {
        let mut value = json!({
            "format_version": 2,
            "document": {
                "id": 3,
                "layers": [{
                    "id": 9,
                    "video": {"source":{"File":{"path":"/media/example.mov"}}},
                    "content": {"kind":"smart","contents_id":12,"source":{"kind":"linked","path":"/media/example.mov"},"children":[]}
                }],
                "timeline": {"audio_tracks":[{"clips":[{"path":"/media/example.mov"}]}]}
            }
        });
        migrate(&mut value).unwrap();
        let document = &value["document"];
        let source_id = document["layers"][0]["video"]["asset_id"].as_u64().unwrap();
        assert!(source_id > 12);
        assert_eq!(document["layers"][0]["content"]["asset_id"], source_id);
        assert_eq!(document["timeline"]["audio_tracks"][0]["clips"][0]["asset_id"], source_id);
        assert_eq!(document["media_assets"].as_array().unwrap().len(), 1);
        assert_eq!(document["media_assets"][0]["path"], "/media/example.mov");
    }
}
