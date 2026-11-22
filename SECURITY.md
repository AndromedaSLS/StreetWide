# Security policy

## Reporting a vulnerability

Please **do not open a public issue** for security problems.

Report vulnerabilities privately through GitHub:
**[Report a vulnerability](https://github.com/AndromedaSLS/StreetWide/security/advisories/new)**
(repository → *Security* → *Report a vulnerability*).

Include what you found, how to reproduce it, and the impact you expect. We
aim to acknowledge reports within 7 days and to agree a disclosure timeline
with you. We credit reporters in the advisory unless you prefer to stay
anonymous.

## Scope

StreetWide is pre-alpha and has no networking or accounts yet. Relevant
issues today include, for example:

- crafted world tiles, manifests or replay files that crash the client or
  tools, or make them read or write outside their data directories;
- ingest scripts that can be tricked into executing downloaded content;
- weaknesses in the content-addressing (hashing) that would let a modified
  asset pass as the original;
- supply-chain risks in our dependencies or CI.

## Supported versions

Only the latest commit on `main` is supported until the first release.
