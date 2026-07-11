# Security Policy

## Supported versions

DiskDeck is currently a single-maintainer project. Security fixes target the latest `main` revision and the latest published GitHub Release once releases begin. Older source snapshots are not maintained separately.

## Reporting a vulnerability

Please do not open a public issue for a suspected vulnerability. Use GitHub's private reporting flow:

[Report a vulnerability privately](https://github.com/raghavaadi/DiskDeck/security/advisories/new)

Include the affected revision, macOS version, reproduction steps, potential impact, and the smallest safe proof you can provide. Remove personal paths, disk contents, credentials, and other private machine data from logs and screenshots.

Security-sensitive areas include:

- any deletion without explicit selection and the confirmation hold;
- command injection or a command assembled from UI-controlled input;
- symlink, path traversal, or volume-boundary mistakes;
- offload verification that could remove an original before proving the copy;
- changes that silently reset Full Disk Access by altering bundle or signing identity;
- exposed credentials, private keys, machine paths, or private commit identity.

Please allow time for investigation and a coordinated fix. As a single-maintainer project, DiskDeck does not promise a fixed response-time SLA.
