//! MIME type matching that tolerates charset parameters.

/// Find a MIME type match in `available`, tolerating charset differences.
///
/// A source may offer `text/plain` while a consumer asks for
/// `text/plain;charset=utf-8`, or the other way round. An exact match wins;
/// for `text/` types the charset parameter is then stripped or added, and
/// finally any offered type with the same base type is accepted.
///
/// Returns the string from `available` that should be used for the request, so
/// that the compositor receives a type the source actually offered.
///
/// ```
/// use lamco_data_control::find_mime_match;
///
/// let available = vec!["text/plain;charset=utf-8".to_owned()];
/// assert_eq!(
///     find_mime_match("text/plain", &available),
///     Some("text/plain;charset=utf-8")
/// );
/// assert_eq!(find_mime_match("image/png", &available), None);
/// ```
#[must_use]
pub fn find_mime_match<'a>(requested: &str, available: &'a [String]) -> Option<&'a str> {
    if let Some(found) = available.iter().find(|m| m.as_str() == requested) {
        return Some(found.as_str());
    }

    if !requested.starts_with("text/") {
        return None;
    }
    let base = requested.split(';').next()?;

    if requested.contains(';') {
        // The request carries a charset; try the bare type.
        if let Some(found) = available.iter().find(|m| m.as_str() == base) {
            return Some(found.as_str());
        }
    } else {
        // The request has none; try the common charset spellings.
        for suffix in [";charset=utf-8", ";charset=UTF-8"] {
            let with_charset = format!("{requested}{suffix}");
            if let Some(found) = available.iter().find(|m| m.as_str() == with_charset) {
                return Some(found.as_str());
            }
        }
    }

    available
        .iter()
        .find(|m| m.split(';').next() == Some(base))
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::find_mime_match;

    fn types(list: &[&str]) -> Vec<String> {
        list.iter().map(|t| (*t).to_owned()).collect()
    }

    #[test]
    fn an_exact_match_is_returned() {
        let available = types(&["text/plain", "text/html"]);
        assert_eq!(find_mime_match("text/html", &available), Some("text/html"));
    }

    #[test]
    fn a_requested_charset_is_stripped() {
        let available = types(&["text/plain"]);
        assert_eq!(
            find_mime_match("text/plain;charset=utf-8", &available),
            Some("text/plain")
        );
    }

    #[test]
    fn a_missing_charset_is_added() {
        let available = types(&["text/plain;charset=utf-8"]);
        assert_eq!(
            find_mime_match("text/plain", &available),
            Some("text/plain;charset=utf-8")
        );
    }

    #[test]
    fn non_text_types_get_no_charset_fallback() {
        let available = types(&["image/png;foo=bar"]);
        assert_eq!(find_mime_match("image/png", &available), None);
    }

    #[test]
    fn an_exact_match_beats_a_charset_variant() {
        let available = types(&["text/plain;charset=utf-8", "text/plain"]);
        assert_eq!(
            find_mime_match("text/plain", &available),
            Some("text/plain")
        );
    }

    #[test]
    fn any_charset_of_the_same_base_type_is_accepted() {
        let available = types(&["text/plain;charset=iso-8859-1"]);
        assert_eq!(
            find_mime_match("text/plain", &available),
            Some("text/plain;charset=iso-8859-1")
        );
    }
}
