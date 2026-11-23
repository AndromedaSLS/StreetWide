"""StreetWide contribution tools (RFC 0002).

    python -m tools.contrib reference --world <dir> --at <lon,lat> --out <dir>
    python -m tools.contrib facade --world <dir> --building <feature-id> --model <file.glb> ...

`reference` finds a building and exports its shell as a glTF file to model
on; `facade` turns a model exported from a 3D editor into a contribution
bundle (a change set with its manifest and mesh) that the gate can check.
See docs/contributing/facades.md.
"""

VERSION = "0.1.0"
GENERATOR = "streetwide-contrib"
