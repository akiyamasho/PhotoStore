//! Native application update commands. The engine owns the stable command surface; a desktop
//! shell supplies the platform-specific GitHub/download/install service.

use serde_json::Value;

use crate::commands::{CommandSpec, always};
use crate::{EngineError, Result, Session};

/// Native callback: method names are `check`, `status`, `prepare`, and `install`.
pub type UpdateService = Box<dyn FnMut(&str, &Value) -> std::result::Result<Value, String> + Send>;

fn check(s: &mut Session, p: &Value) -> Result<Value> {
    invoke(s, "app.update.check", "check", p)
}
fn status(s: &mut Session, p: &Value) -> Result<Value> {
    invoke(s, "app.update.status", "status", p)
}
fn prepare(s: &mut Session, p: &Value) -> Result<Value> {
    invoke(s, "app.update.prepare", "prepare", p)
}
fn install(s: &mut Session, p: &Value) -> Result<Value> {
    invoke(s, "app.update.install", "install", p)
}

fn invoke(s: &mut Session, id: &str, method: &str, params: &Value) -> Result<Value> {
    if !params.as_object().is_some_and(|o| o.is_empty()) {
        return Err(EngineError::BadParams { cmd: id.into(), msg: "no parameters are accepted".into() });
    }
    if method == "install" {
        if !s.documents().is_empty() {
            return Err(EngineError::Disabled(id.into(), "close all document tabs before installing the update".into()));
        }
        if s.has_jobs() {
            return Err(EngineError::Disabled(id.into(), "wait for all background jobs to finish before installing the update".into()));
        }
    }
    let service = s.update_service.as_mut().ok_or_else(|| EngineError::Other("Native update support is unavailable in this app build".into()))?;
    service(method, params).map_err(EngineError::Other)
}

pub fn specs() -> Vec<CommandSpec> {
    [
        CommandSpec {
            id: "app.update.check",
            label: "Check for Updates…",
            run: check,
            enabled: always,
            menu: &[],
            shortcut: None,
            params: "{}",
            journal: false,
        },
        CommandSpec { id: "app.update.status", label: "Update Status", run: status, enabled: always, menu: &[], shortcut: None, params: "{}", journal: false },
        CommandSpec {
            id: "app.update.prepare",
            label: "Download Update",
            run: prepare,
            enabled: always,
            menu: &[],
            shortcut: None,
            params: "{}",
            journal: false,
        },
        CommandSpec {
            id: "app.update.install",
            label: "Install Update and Quit",
            run: install,
            enabled: install_enabled,
            menu: &[],
            shortcut: None,
            params: "{}",
            journal: false,
        },
    ]
    .into()
}

fn install_enabled(s: &Session) -> std::result::Result<(), String> {
    if !s.documents().is_empty() {
        return Err("close all document tabs before installing the update".into());
    }
    if s.has_jobs() {
        return Err("wait for all background jobs to finish before installing the update".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn commands_require_empty_params_and_native_service() {
        let mut s = Session::new();
        let check = specs().into_iter().find(|c| c.id == "app.update.check").unwrap();
        assert!(matches!((check.run)(&mut s, &json!({})), Err(EngineError::Other(_))));
        assert!(matches!((check.run)(&mut s, &json!({"unexpected": true})), Err(EngineError::BadParams { .. })));
        s.update_service = Some(Box::new(|method, _| Ok(json!({"method": method}))));
        assert_eq!((check.run)(&mut s, &json!({})).unwrap(), json!({"method":"check"}));
    }

    #[test]
    fn install_refuses_any_open_document() {
        let mut s = Session::new();
        s.update_service = Some(Box::new(|_, _| Ok(json!({"state":"installing","quit":true}))));
        s.add_document(
            crate::doc::Document::new("Untitled", crate::doc::Size::new(1, 1), photostore_color::ColorMode::Rgb, photostore_color::SampleType::U8),
            None,
        );
        let install = specs().into_iter().find(|c| c.id == "app.update.install").unwrap();
        assert!(matches!((install.run)(&mut s, &json!({})), Err(EngineError::Disabled(_, _))));
    }
}
