//! Canonical JSON, the byte form TUF signatures cover.
//!
//! TUF signs the `securesystemslib` (historically OLPC) canonical form, not RFC
//! 8785 JCS: keys sorted by code point, no insignificant whitespace, integers
//! only, and only `\` and `"` escaped in strings. The difference is not
//! cosmetic — a PEM blob's embedded newlines stay raw bytes here and would be
//! escaped under JCS, so every signature would fail against real repositories.

use olpc_cjson::CanonicalFormatter;
use serde::Serialize;

use crate::error::Result;

/// Serialize `value` to canonical JSON bytes.
pub fn to_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut buf, CanonicalFormatter::new());
    value.serialize(&mut serializer)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorts_keys_and_strips_whitespace() {
        assert_eq!(
            to_bytes(&json!({ "b": 1, "a": 2 })).unwrap(),
            b"{\"a\":2,\"b\":1}"
        );
    }

    #[test]
    fn newlines_stay_raw() {
        let bytes = to_bytes(&json!({ "public": "line1\nline2" })).unwrap();
        assert_eq!(bytes, b"{\"public\":\"line1\nline2\"}");
        assert!(!bytes.windows(2).any(|w| w == b"\\n"));
    }
}
