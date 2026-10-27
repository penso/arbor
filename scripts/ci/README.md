# macOS release credentials

Release signing uses `MACOS_CERTIFICATE` (base64 Developer ID Application `.p12`)
and `MACOS_CERTIFICATE_PASSWORD` (the password protecting that certificate).

Notarization uses a team App Store Connect API key with these GitHub Actions secrets:

| Secret | Value |
| --- | --- |
| `APPLE_API_PRIVATE_KEY` | Raw multiline contents of the downloaded `.p8` key |
| `APPLE_API_KEY_ID` | App Store Connect key ID |
| `APPLE_API_ISSUER_ID` | Issuer UUID for the team that created the key |

The API key must belong to the signing certificate's team. It replaces Apple ID
authentication, not the signing certificate. No Apple ID password is needed.

`notarize-macos.sh` writes the key to a private temporary file, removes it on exit,
and requires a successful submission command and an `Accepted` status before the
workflow staples the ticket. Failed submissions fetch Apple's diagnostic log.

Run `bash scripts/ci/test-notarize-macos.sh` for mocked authentication, cleanup,
and failure-path tests. Validate API credentials separately with
`xcrun notarytool history --key /secure/path/AuthKey.p8 --key-id KEY_ID --issuer ISSUER_UUID`.
A signed release submission is needed to verify the full notarization flow.
