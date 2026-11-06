# Auto-update manifest

The updater reads `manifest.json` from the latest GitHub Release:

`https://github.com/Tzigan/FileManager/releases/latest/download/manifest.json`

Schema version 1 contains the archive version, HTTPS release URL, SHA-256 and
exact archive size. The workflow writes the real hash and size; the example
values below are placeholders:

```json
{
  "schema_version": 1,
  "version": "0.7.0",
  "archive_url": "https://github.com/Tzigan/FileManager/releases/download/v0.7.0/FileManager-v0.7.0-x64.zip",
  "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
  "size": 12345678
}
```

`minimum_supported_version` is optional. If present, clients older than that
version refuse the automatic update. The release workflow signs both executables
with Authenticode; the updater verifies both signatures before installation.
The optional `signature` field is reserved for a detached manifest signature
and is currently omitted.

The workflow reads the PFX and password from the GitHub Actions secrets
`WINDOWS_SIGNING_PFX_BASE64` and `WINDOWS_SIGNING_PFX_PASSWORD`. The published
`v0.6.6` binaries predate this signing step; the next release will be signed.

The archive must contain exactly `FileManager.exe` and
`FileManagerUpdater.exe` at its root. `SHA256SUMS.txt` is published beside the
archive with hashes computed for both executables before compression, the legacy
standalone `.exe`, and the final archive. Keeping these checksums outside the
ZIP preserves compatibility with the updater in v0.6.6. The release workflow
also publishes the legacy standalone `.exe` asset so older clients can migrate.
