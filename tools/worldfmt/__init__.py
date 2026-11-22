"""Python implementation of the StreetWide world format (RFC 0001).

Byte-for-byte compatible with the Rust reference implementation in
`crates/op_world`: canonical JSON (`cjson`), content ids (`ids`), the glb
profile writer (`glb`) and JSON Schema validation (`schemas`). The tests in
`tools/worldfmt/tests` check this against the Rust-generated golden fixture
in `fixtures/format/`.
"""

CONTRACT_VERSION = "0.1.0"
