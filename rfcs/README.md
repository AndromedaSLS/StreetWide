# RFCs

An RFC proposes a change to a **contract** that other people build against:

- the world format (asset manifests, tiles, hashing);
- module interfaces in `op_kernel`;
- the replay format;
- the rules that decide whether a contribution may enter the canonical world.

Process (see [GOVERNANCE.md](../GOVERNANCE.md)):

1. Copy [0000-template.md](0000-template.md) to `NNNN-short-name.md` (next
   free number) and open a pull request marked *Draft*.
2. The RFC stays open for comment for at least 7 days.
3. Maintainers accept or reject it and record the decision and reasons in
   the RFC. Accepted RFCs are merged; implementation can then start.

| RFC | Title | Status |
|---|---|---|
| [0001](0001-world-format.md) | World format | Draft |
| [0002](0002-contributions-and-merge-rules.md) | Contributions and merge rules | Draft |
| [0003](0003-water-layer.md) | A water layer | Draft |
