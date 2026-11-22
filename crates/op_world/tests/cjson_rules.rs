//! Canonical JSON (RFC 0001, section 5): exactly one byte form per value.

use op_world::Error;
use op_world::cjson::{from_canonical, to_vec};
use serde_json::{Value, json};

fn parse(bytes: &[u8]) -> Result<Value, Error> {
    from_canonical::<Value>(bytes)
}

#[test]
fn canonical_documents_round_trip() {
    // A backslash, spelled out so the JSON escapes below stay visible.
    let bs = char::from(92u8);
    let text = format!(
        r#"{{"a":"{bs}u00e9 {bs}"q{bs}" {bs}{bs} {bs}u00a9","b":[1,-2,null,true,false],"c":{{"d":{{}}}}}}"#
    );
    let doc = text.as_bytes();
    // The literal above uses JSON escapes for readability; build the real
    // canonical bytes by serialising the value.
    let value: Value = serde_json::from_slice(doc).unwrap();
    let canonical = to_vec(&value).unwrap();
    assert_eq!(
        String::from_utf8(canonical.clone()).unwrap(),
        "{\"a\":\"é \\\"q\\\" \\\\ ©\",\"b\":[1,-2,null,true,false],\"c\":{\"d\":{}}}"
    );
    assert_eq!(parse(&canonical).unwrap(), value);
}

#[test]
fn keys_are_sorted_by_bytes() {
    let v = json!({"b": 1, "a": 2, "a_b": 3, "a0": 4});
    assert_eq!(to_vec(&v).unwrap(), br#"{"a":2,"a0":4,"a_b":3,"b":1}"#);
}

#[test]
fn non_canonical_forms_are_rejected() {
    let bs = char::from(92u8);
    let escaped_non_ascii = format!(r#"{{"a":"{bs}u00e9"}}"#);
    let cases: Vec<(&str, &[u8])> = vec![
        ("whitespace", br#"{"a": 1}"#),
        ("unsorted keys", br#"{"b":1,"a":2}"#),
        ("float", br#"{"a":1.0}"#),
        ("exponent", br#"{"a":1e3}"#),
        ("negative zero", br#"{"a":-0}"#),
        ("byte-order mark", b"\xEF\xBB\xBF{\"a\":1}"),
        ("trailing newline", b"{\"a\":1}\n"),
        ("duplicate key", br#"{"a":1,"a":1}"#),
        ("escaped non-ASCII", escaped_non_ascii.as_bytes()),
        ("escaped slash", br#"{"a":"\/"}"#),
        ("control character escape", br#"{"a":"x\ny"}"#),
        ("uppercase key", br#"{"A":1}"#),
        ("integer above 2^53-1", br#"{"a":9007199254740992}"#),
        ("integer below -(2^53-1)", br#"{"a":-9007199254740992}"#),
    ];
    for (name, bytes) in &cases {
        assert!(parse(bytes).is_err(), "{name} should be rejected");
    }
}

#[test]
fn largest_safe_integers_are_accepted() {
    assert!(parse(br#"{"a":9007199254740991,"b":-9007199254740991}"#).is_ok());
}

#[test]
fn missing_optional_field_is_not_canonical() {
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Doc {
        a: Option<u32>,
    }
    assert!(from_canonical::<Doc>(br#"{"a":null}"#).is_ok());
    assert!(matches!(
        from_canonical::<Doc>(b"{}"),
        Err(Error::NotCanonical(_))
    ));
}
