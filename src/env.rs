//! Environment configuration with compatibility for the original prefix.

use std::{env::VarError, ffi::OsString};

fn select(primary: Option<OsString>, legacy: Option<OsString>) -> Option<(OsString, bool)> {
    primary
        .map(|value| (value, false))
        .or_else(|| legacy.map(|value| (value, true)))
}

fn configured_value(suffix: &str) -> Option<(OsString, bool)> {
    select(
        std::env::var_os(format!("XDP_KNAVE_{suffix}")),
        std::env::var_os(format!("XDP_GENERIC_{suffix}")),
    )
}

/// Read a Knave-prefixed setting, falling back to its former generic name.
pub(crate) fn var(suffix: &str) -> Result<String, VarError> {
    let Some((value, _legacy)) = configured_value(suffix) else {
        return Err(VarError::NotPresent);
    };
    value.into_string().map_err(VarError::NotUnicode)
}

/// Check whether a setting is present under either supported prefix.
#[cfg(test)]
pub(crate) fn is_set(suffix: &str) -> bool {
    configured_value(suffix).is_some()
}

/// Warn once at startup for each deprecated setting that is actually in use.
pub(crate) fn warn_on_legacy_settings() {
    const SETTINGS: &[&str] = &[
        "CAPTURE_PROTOCOL",
        "CAPTURE_NO_FALLBACK",
        "CAPTURE_TIMEOUT_MS",
        "INPUT_PROTOCOL",
        "INPUT_NO_FALLBACK",
        "EIS_SOCKET",
        "CLIPBOARD_PROTOCOL",
        "CLIPBOARD_NO_FALLBACK",
        "SOURCE_PICKER",
        "COLOR_PICKER",
        "COLOR_SCHEME",
        "ACCENT_COLOR",
        "CONTRAST",
        "REDUCED_MOTION",
    ];
    for suffix in SETTINGS {
        let primary = format!("XDP_KNAVE_{suffix}");
        let legacy = format!("XDP_GENERIC_{suffix}");
        if std::env::var_os(&primary).is_none() && std::env::var_os(&legacy).is_some() {
            tracing::warn!(variable = %legacy, replacement = %primary, "deprecated portal environment variable is in use");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knave_setting_takes_precedence_over_legacy_setting() {
        let selected = select(
            Some(OsString::from("knave")),
            Some(OsString::from("legacy")),
        );
        assert_eq!(selected, Some((OsString::from("knave"), false)));
    }

    #[test]
    fn legacy_setting_is_used_when_knave_setting_is_absent() {
        let selected = select(None, Some(OsString::from("legacy")));
        assert_eq!(selected, Some((OsString::from("legacy"), true)));
    }

    #[test]
    fn missing_setting_stays_unset() {
        assert_eq!(select(None, None), None);
    }
}
