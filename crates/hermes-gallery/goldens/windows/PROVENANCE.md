# Windows gallery reference provenance

- Source: `1153f56a9e82c49447b2f8dba4802e2ace3f1acd`.
- Platform: GitHub-hosted `windows-latest`, Slint testing backend + software renderer, mocked time. These are not Linux renders renamed as Windows images.
- Run: https://github.com/cynricantao/hermes-desktop-rewrite/actions/runs/39581140968
- Artifact: `gallery-windows-1153f56a9e82c49447b2f8dba4802e2ace3f1acd`, ID `12338795391`.
- Downloaded archive SHA-256: `83466642a24cfae8865a025b0796b14b868487cba8a14d41760e526d7437b635`, matching GitHub's artifact digest.
- Verified 14 PNG members, RGBA, 1440x900, without links or unexpected paths. Inspected the board contact sheet and full-size controls/dialog views before replacing references.

The Windows interaction/helper tests passed before capture. The old references failed comparison on tokens-light/dark and controls-light/dark because the intentional contrast/focus changes exceeded the unchanged 0.2% tolerance. Other differences are the corresponding badge, focus and semantic-colour changes. No comparison thresholds were weakened.

Production Rust/Slint code is unchanged by this reference refresh. A follow-up Windows run compares all 14 committed references against fresh renders.
