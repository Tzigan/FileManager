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
version refuse the automatic update. `signature` is also reserved but omitted:
the current release pipeline does not sign executables. A manifest containing a
signature is rejected until signature verification is configured.

The archive must contain exactly `FileManager.exe` and
`FileManagerUpdater.exe` at its root. The release workflow also publishes the
legacy standalone `.exe` asset so versions using the old updater can migrate.
