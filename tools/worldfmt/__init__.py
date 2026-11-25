"""Python implementation of the StreetWide world format (RFC 0001) and of
the contribution documents (RFC 0002).

Byte-for-byte compatible with the Rust reference implementation in
`crates/op_world`: canonical JSON (`cjson`), content ids (`ids`), the glb
profile writer and strict reader (`glb`), points documents, the content
of assets in `vegetation` and `street_objects` (`points`, RFC 0004), JSON
Schema validation (`schemas`), the store layout (`store`) and the size
limit of each kind of JSON document (`limits`). For change sets and the world log: the
change-set rules and the candidate world a set produces (`changes`), and
a world log kept in a directory (`log`). The tests in
`tools/worldfmt/tests` check this against the Rust-generated golden
fixture in `fixtures/format/` and against ids that op_world made.
"""

CONTRACT_VERSION = "0.1.0"
