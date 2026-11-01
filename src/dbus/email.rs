//! Email D-Bus interface implementation.
//!
//! Implements `org.freedesktop.impl.portal.Email` by opening a mailto URI in
//! the host's default email application.

use std::{collections::HashMap, time::Duration};

use zbus::{
    interface,
    zvariant::{ObjectPath, OwnedValue, Value},
};

use super::Response;

/// Email portal interface implementation.
pub struct EmailInterface;

#[interface(name = "org.freedesktop.impl.portal.Email", introspection_docs = false)]
impl EmailInterface {
    /// Open a compose window in the host's default email application.
    #[zbus(name = "ComposeEmail")]
    async fn compose_email(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        parent_window: &str,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let _ = (handle, app_id, parent_window);
        let uri = mailto_uri(&options)?;
        let mut child = tokio::process::Command::new("xdg-open")
            .arg(&uri)
            .spawn()
            .map_err(|error| {
                zbus::fdo::Error::Failed(format!("Could not open the email application: {error}"))
            })?;
        // Give xdg-open a brief window to report dispatch errors, without
        // waiting for the user's mail application to exit.
        check_launcher_status(&mut child).await?;

        Ok((Response::Success.to_u32(), HashMap::new()))
    }
}

/// Report fast launcher failures while allowing a long-running mail client to stay open.
async fn check_launcher_status(child: &mut tokio::process::Child) -> zbus::fdo::Result<()> {
    match tokio::time::timeout(Duration::from_millis(250), child.wait()).await {
        Ok(Ok(status)) if !status.success() => Err(zbus::fdo::Error::Failed(format!(
            "The email application exited with {status}"
        ))),
        Ok(Err(error)) => Err(zbus::fdo::Error::Failed(format!(
            "Could not check the email application launch: {error}"
        ))),
        Ok(Ok(_)) | Err(_) => Ok(()),
    }
}

/// Build a bounded `mailto:` URI from the portal's typed compose options.
fn mailto_uri(options: &HashMap<String, OwnedValue>) -> zbus::fdo::Result<String> {
    let mut to = Vec::new();
    if let Some(address) = option_string(options, "address")? {
        to.push(address.to_owned());
    }
    to.extend(option_strings(options, "addresses")?);

    let mut uri = String::from("mailto:");
    uri.push_str(
        &to.iter()
            .map(|address| encode_address(address))
            .collect::<Vec<_>>()
            .join(","),
    );

    let mut query = Vec::new();
    for key in ["cc", "bcc"] {
        let addresses = option_strings(options, key)?;
        if !addresses.is_empty() {
            query.push(format!(
                "{key}={}",
                addresses
                    .iter()
                    .map(|address| encode_address(address))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
    }
    for key in ["subject", "body"] {
        if let Some(value) = option_string(options, key)? {
            query.push(format!("{key}={}", encode_component(value)));
        }
    }
    for attachment in option_strings(options, "attachments")? {
        query.push(format!("attachment={}", encode_component(&attachment)));
    }
    if !query.is_empty() {
        uri.push('?');
        uri.push_str(&query.join("&"));
    }
    if uri.len() > 64 * 1024 {
        return Err(zbus::fdo::Error::InvalidArgs(
            "Email request exceeds the 64 KiB URI limit".into(),
        ));
    }
    Ok(uri)
}

/// Read an optional string option, rejecting values with a different D-Bus type.
fn option_string<'a>(
    options: &'a HashMap<String, OwnedValue>,
    key: &str,
) -> zbus::fdo::Result<Option<&'a str>> {
    let Some(value) = options.get(key) else {
        return Ok(None);
    };
    let value: &Value<'_> = value
        .downcast_ref()
        .map_err(|_| invalid_option(key, "string"))?;
    match value {
        Value::Str(value) => Ok(Some(value.as_str())),
        _ => Err(invalid_option(key, "string")),
    }
}

/// Read an optional string-array option, rejecting arrays with non-string values.
fn option_strings(
    options: &HashMap<String, OwnedValue>,
    key: &str,
) -> zbus::fdo::Result<Vec<String>> {
    let Some(value) = options.get(key) else {
        return Ok(Vec::new());
    };
    let value: &Value<'_> = value
        .downcast_ref()
        .map_err(|_| invalid_option(key, "array of strings"))?;
    let Value::Array(values) = value else {
        return Err(invalid_option(key, "array of strings"));
    };
    values
        .iter()
        .map(|value| match value {
            Value::Str(value) => Ok(value.to_string()),
            _ => Err(invalid_option(key, "array of strings")),
        })
        .collect()
}

fn invalid_option(key: &str, expected: &str) -> zbus::fdo::Error {
    zbus::fdo::Error::InvalidArgs(format!("Email option '{key}' must be a {expected}"))
}

fn encode_address(address: &str) -> String {
    encode_component_with(address, b"@,+")
}

fn encode_component(value: &str) -> String {
    encode_component_with(value, b"")
}

/// Percent-encode a URI component while preserving the supplied safe bytes.
fn encode_component_with(value: &str, extra_safe: &[u8]) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) || extra_safe.contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(value: &str) -> OwnedValue {
        OwnedValue::try_from(Value::from(value)).unwrap()
    }

    fn strings(values: &[&str]) -> OwnedValue {
        let values: Vec<&str> = values.to_vec();
        OwnedValue::try_from(Value::from(values)).unwrap()
    }

    #[test]
    fn builds_mailto_uri_with_recipients_and_headers() {
        let options = HashMap::from([
            ("address".into(), string("one@example.com")),
            ("addresses".into(), strings(&["two@example.com"])),
            ("cc".into(), strings(&["copy@example.com"])),
            ("subject".into(), string("Hello there")),
            ("body".into(), string("Line 1\nLine 2")),
            ("attachments".into(), strings(&["file:///tmp/a b.txt"])),
        ]);

        assert_eq!(
            mailto_uri(&options).unwrap(),
            "mailto:one@example.com,two@example.com?cc=copy@example.com&subject=Hello%20there&body=Line%201%0ALine%202&attachment=file%3A%2F%2F%2Ftmp%2Fa%20b.txt"
        );
    }

    #[test]
    fn rejects_malformed_option_types() {
        let options = HashMap::from([("subject".into(), OwnedValue::from(42_u32))]);
        assert!(matches!(
            mailto_uri(&options),
            Err(zbus::fdo::Error::InvalidArgs(_))
        ));
    }

    #[test]
    fn omits_empty_options() {
        assert_eq!(mailto_uri(&HashMap::new()).unwrap(), "mailto:");
    }

    #[tokio::test]
    async fn reports_a_fast_launcher_failure() {
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", "exit 7"])
            .spawn()
            .unwrap();

        assert!(matches!(
            check_launcher_status(&mut child).await,
            Err(zbus::fdo::Error::Failed(_))
        ));
    }

    #[tokio::test]
    async fn returns_while_a_mail_client_remains_open() {
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", "sleep 2"])
            .spawn()
            .unwrap();

        assert!(
            tokio::time::timeout(Duration::from_secs(1), check_launcher_status(&mut child))
                .await
                .unwrap()
                .is_ok()
        );
        child.kill().await.unwrap();
        let _ = child.wait().await;
    }
}
