use std::env;

const MAX_GOAL_LEN: usize = 48;

fn validate_goal(raw: &str) -> Result<(), String> {
    if raw.is_empty() {
        return Ok(());
    }
    if raw.len() > MAX_GOAL_LEN {
        return Err(format!("PHOTOSTORE_DEV_GOAL is too long ({} bytes; maximum is {MAX_GOAL_LEN})", raw.len()));
    }
    let bytes = raw.as_bytes();
    if !bytes[0].is_ascii_lowercase() {
        return Err("PHOTOSTORE_DEV_GOAL must start with a lowercase ASCII letter".into());
    }
    if !bytes[bytes.len() - 1].is_ascii_lowercase() && !bytes[bytes.len() - 1].is_ascii_digit() {
        return Err("PHOTOSTORE_DEV_GOAL must end with a lowercase ASCII letter or digit".into());
    }
    if !bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-') {
        return Err("PHOTOSTORE_DEV_GOAL may contain only lowercase ASCII letters, digits, and hyphens".into());
    }
    Ok(())
}

fn generated_identity(goal: &str) -> (String, String, String, String, String, String) {
    if goal.is_empty() {
        (
            "PhotoStore-dev".into(),
            "ai.photostore.app.dev".into(),
            "photostore-dev".into(),
            "portable-dev.txt".into(),
            "PhotoStore-dev.portable".into(),
            "PhotoStoreDevData".into(),
        )
    } else {
        (
            format!("PhotoStore-dev-{goal}"),
            format!("ai.photostore.app.dev.{goal}"),
            format!("photostore-dev-{goal}"),
            format!("portable-dev-{goal}.txt"),
            format!("PhotoStore-dev-{goal}.portable"),
            format!("PhotoStoreDevData-{goal}"),
        )
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=PHOTOSTORE_DEV_BUILD");
    println!("cargo:rerun-if-env-changed=PHOTOSTORE_DEV_GOAL");
    let goal = match env::var("PHOTOSTORE_DEV_GOAL") {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => String::new(),
        Err(env::VarError::NotUnicode(_)) => panic!("invalid PHOTOSTORE_DEV_GOAL: value is not valid Unicode"),
    };
    if let Err(error) = validate_goal(&goal) {
        panic!("invalid PHOTOSTORE_DEV_GOAL `{goal}`: {error}");
    }
    let (app_name, app_id, linux_name, marker_one, marker_two, data_dir) = generated_identity(&goal);
    for (key, value) in [
        ("PHOTOSTORE_GENERATED_APP_NAME", app_name.to_string()),
        ("PHOTOSTORE_GENERATED_APP_ID", app_id),
        ("PHOTOSTORE_GENERATED_LINUX_CONFIG", linux_name),
        ("PHOTOSTORE_GENERATED_PORTABLE_MARKER_ONE", marker_one),
        ("PHOTOSTORE_GENERATED_PORTABLE_MARKER_TWO", marker_two),
        ("PHOTOSTORE_GENERATED_PORTABLE_DATA_DIR", data_dir),
    ] {
        println!("cargo:rustc-env={key}={value}");
    }
}

#[cfg(test)]
mod tests {
    use super::{generated_identity, validate_goal};

    #[test]
    fn consolidated_identity_is_stable() {
        let (name, id, linux, marker_one, marker_two, data) = generated_identity("");
        assert_eq!((name, id, linux, marker_one, marker_two, data), ("PhotoStore-dev".into(), "ai.photostore.app.dev".into(), "photostore-dev".into(), "portable-dev.txt".into(), "PhotoStore-dev.portable".into(), "PhotoStoreDevData".into()));
    }

    #[test]
    fn goal_identity_is_independent() {
        let (name, id, linux, marker_one, marker_two, data) = generated_identity("timeline-safety");
        assert_eq!((name, id, linux, marker_one, marker_two, data), ("PhotoStore-dev-timeline-safety".into(), "ai.photostore.app.dev.timeline-safety".into(), "photostore-dev-timeline-safety".into(), "portable-dev-timeline-safety.txt".into(), "PhotoStore-dev-timeline-safety.portable".into(), "PhotoStoreDevData-timeline-safety".into()));
    }

    #[test]
    fn rejects_traversal_uppercase_and_oversized_goals() {
        assert!(validate_goal("../secret").is_err());
        assert!(validate_goal("Timeline-safety").is_err());
        assert!(validate_goal(&"a".repeat(49)).is_err());
    }
}
