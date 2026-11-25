//! Canonical JSON (RFC 0001, section 5).
//!
//! A strict subset of JSON with exactly one byte representation per value:
//! UTF-8, no whitespace, keys sorted by byte value, integers only (within
//! ±(2^53 − 1)), no control characters, and only `\"` and `\\` as escapes.
//! Readers reject any document whose bytes differ from that serialisation.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{Error, Result};

/// Largest integer exactly representable as an IEEE 754 double.
pub const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

/// Deepest nesting of arrays and objects, the outermost value being level 1
/// (RFC 0001, section 5). It is serde_json's own limit (its recursion limit
/// is 128), checked here so that the rule does not depend on a parser
/// setting; `tools/worldfmt` enforces the same.
pub const MAX_DEPTH: usize = 127;

/// Serialises a JSON value canonically.
pub fn to_vec(value: &Value) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(256);
    write_value(value, &mut out)?;
    Ok(out)
}

/// Serialises any serde value canonically.
pub fn to_canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    to_vec(&serde_json::to_value(value)?)
}

/// Parses canonical JSON into `T`, rejecting non-canonical bytes.
///
/// The check runs twice: on the raw JSON value (whitespace, key order,
/// number and string forms, duplicate keys) and on the typed value, so a
/// field that was omitted and filled in by a default (such as a missing
/// `null`) is also rejected.
pub fn from_canonical<T: Serialize + DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    check_depth(bytes)?;
    let value: Value = serde_json::from_slice(bytes)?;
    let canonical = to_vec(&value)?;
    if canonical != bytes {
        return Err(Error::NotCanonical(first_difference(bytes, &canonical)));
    }
    let typed: T = serde_json::from_value(value)?;
    if to_canonical(&typed)? != bytes {
        return Err(Error::NotCanonical(
            "document omits fields that must be written explicitly".into(),
        ));
    }
    Ok(typed)
}

/// Refuses JSON text whose arrays and objects nest deeper than
/// [`MAX_DEPTH`], before it is parsed. Brackets inside strings do not count;
/// anything else that is not JSON is left for the parser to refuse.
pub(crate) fn check_depth(bytes: &[u8]) -> Result<()> {
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    for &b in bytes {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(Error::NotCanonical(format!(
                        "arrays and objects nested deeper than {MAX_DEPTH} levels"
                    )));
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

fn first_difference(actual: &[u8], canonical: &[u8]) -> String {
    let at = actual
        .iter()
        .zip(canonical)
        .position(|(a, b)| a != b)
        .unwrap_or(actual.len().min(canonical.len()));
    format!(
        "bytes differ from the canonical form at offset {at} (document {} bytes, canonical {} bytes)",
        actual.len(),
        canonical.len()
    )
}

fn write_value(value: &Value, out: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => {
            let Some(i) = n.as_i64() else {
                return Err(if n.as_u64().is_some() {
                    Error::Invalid(format!("integer {n} is outside ±(2^53 - 1)"))
                } else {
                    Error::Invalid(format!(
                        "number {n} is not an integer; canonical JSON allows integers only"
                    ))
                });
            };
            if !(-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&i) {
                return Err(Error::Invalid(format!(
                    "integer {i} is outside ±(2^53 - 1)"
                )));
            }
            out.extend_from_slice(i.to_string().as_bytes());
        }
        Value::String(s) => write_string(s, out)?,
        Value::Array(items) => {
            out.push(b'[');
            for (k, item) in items.iter().enumerate() {
                if k > 0 {
                    out.push(b',');
                }
                write_value(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            out.push(b'{');
            for (k, (key, item)) in entries.into_iter().enumerate() {
                check_key(key)?;
                if k > 0 {
                    out.push(b',');
                }
                write_string(key, out)?;
                out.push(b':');
                write_value(item, out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

/// Object keys must match `[a-z][a-z0-9_]*`.
fn check_key(key: &str) -> Result<()> {
    let mut chars = key.chars();
    let ok = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "object key {key:?} does not match [a-z][a-z0-9_]*"
        )))
    }
}

/// Writes a JSON string: escapes `"` and `\`, rejects control characters,
/// writes everything else as raw UTF-8.
pub(crate) fn write_string(s: &str, out: &mut Vec<u8>) -> Result<()> {
    out.push(b'"');
    for ch in s.chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            c if is_control(c) => {
                return Err(Error::Invalid(format!(
                    "string contains control character U+{:04X}",
                    u32::from(c)
                )));
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
    Ok(())
}

pub(crate) fn is_control(c: char) -> bool {
    u32::from(c) < 0x20 || c == '\u{7f}'
}
