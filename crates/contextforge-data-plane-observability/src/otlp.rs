use std::collections::HashMap;

use tonic::metadata::{MetadataKey, MetadataMap, MetadataValue};

/// Converts a header [`HashMap`] to a tonic [`MetadataMap`] for gRPC metadata
/// attachment. Returns an error (and aborts startup) for any entry whose key
/// or value cannot be encoded as valid ASCII gRPC metadata, so invalid
/// configuration is surfaced rather than silently dropped.
pub(crate) fn headers_to_metadata(
    headers: &HashMap<String, String>,
) -> Result<MetadataMap, Box<dyn std::error::Error + Send + Sync>> {
    let mut map = MetadataMap::new();
    for (k, v) in headers {
        let key = MetadataKey::from_bytes(k.as_bytes()).map_err(|e| format!("invalid gRPC metadata key {k:?}: {e}"))?;
        let val = MetadataValue::try_from(v.as_str())
            .map_err(|e| format!("invalid gRPC metadata value for key {k:?}: {e}"))?;
        map.insert(key, val);
    }
    Ok(map)
}

/// Parses a comma-separated `key=value` header string into a [`HashMap`].
///
/// Whitespace around keys and values is trimmed. Empty segments (from
/// trailing commas or whitespace-only entries) are silently skipped.
/// Any non-empty segment that is missing the `=` separator, or has an
/// empty key, is treated as a configuration error so the application
/// fails fast rather than silently dropping user-supplied headers.
pub(crate) fn parse_otlp_headers(
    raw: Option<&str>,
) -> Result<HashMap<String, String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut out = HashMap::new();
    let Some(raw) = raw else { return Ok(out) };
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        match entry.split_once('=') {
            None => {
                return Err(format!("malformed OTLP header entry (missing '=' separator): {entry:?}").into());
            },
            Some((key, _)) if key.trim().is_empty() => {
                return Err(format!("malformed OTLP header entry (empty key): {entry:?}").into());
            },
            Some((key, value)) => {
                out.insert(key.trim().to_owned(), value.trim().to_owned());
            },
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::parse_otlp_headers;

    #[test]
    fn parse_otlp_headers_handles_empty_and_missing_input() {
        assert!(parse_otlp_headers(None).unwrap().is_empty());
        assert!(parse_otlp_headers(Some("")).unwrap().is_empty());
        assert!(parse_otlp_headers(Some(" , , ")).unwrap().is_empty());
    }

    #[test]
    fn parse_otlp_headers_parses_multiple_entries_and_trims_whitespace() {
        let parsed = parse_otlp_headers(Some(" Authorization = Basic abc , X-Project=demo ")).unwrap();
        assert_eq!(parsed.get("Authorization"), Some(&"Basic abc".to_owned()));
        assert_eq!(parsed.get("X-Project"), Some(&"demo".to_owned()));
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn parse_otlp_headers_rejects_entry_without_separator() {
        assert!(parse_otlp_headers(Some("no-equals")).is_err());
        assert!(parse_otlp_headers(Some("good=value,no-equals")).is_err());
    }

    #[test]
    fn parse_otlp_headers_rejects_entry_with_empty_key() {
        assert!(parse_otlp_headers(Some("=missing-key")).is_err());
    }
}
