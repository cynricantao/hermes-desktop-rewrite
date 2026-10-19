# Pets API (`api::pets`)

Typed client for the backend's desktop-pet feature area: every `pet.*` JSON-RPC method and
the three `pet.*` notifications. UI-free — the controller feeds the floating pet, the pet
picker and the generate/hatch flow from these types.

Sources of truth: `tui_gateway/methods_session.py` (`@_pet_method` handlers),
`tui_gateway/server.py::_pet_sprite_payload`, `tui_gateway/contracts/projects_pets.py`,
`tui_gateway/contracts/billing_delegation_pets.py`, `tui_gateway/contracts/events.py`, and
the Electron calls in `apps/desktop/src/store/pet-gallery.ts`,
`apps/desktop/src/store/pet-generate.ts`, `apps/desktop/src/components/pet/floating-pet.tsx`.

Pets are **per profile**: the profile-scoped methods take `profile: &str` (`""` = launch
profile, the param is omitted) exactly like the Electron `petRpc` chokepoint.
`pet.cancel`, `pet.generate` and `pet.hatch` are not profile-scoped by the gateway and take
no `profile` argument here.

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Active pet (renderer payload) | `pet_info_call` / `pet_info` | RPC `pet.info` `{profile?, knownRevision?}` | Local read. **Multi-MB** `spritesheet_base64`; pass the revision you hold as `known_revision` and an unchanged sheet answers `spritesheet_unchanged: true` with the bytes elided. `enabled:false` = display off or pet missing. |
| Active pet metadata | `pet_info_meta_call` / `pet_info_meta` | RPC `pet.info.meta` `{profile?}` | Cheap poll shape: `enabled`, `slug`, `display_name`, `scale`, `spritesheet_revision`. |
| Cell frames | `pet_cells_call` / `pet_cells` | RPC `pet.cells` `{profile?, state?, cols?, graphics?}` | Local render. Unicode frames are `frame → row → cell(8 u8)`; `graphics:true` asks for the kitty payload (`frames` = escapes, `placeholder` grid). `frames` is a wire union, modelled as `Value`. |
| Gallery | `pet_gallery_call` / `pet_gallery` | RPC `pet.gallery` `{profile?, localOnly?}` | **Network**: fetches the remote Petdex manifest; `local_only` skips the inline fetch but still prefetches it on a daemon thread. |
| Adopt | `pet_select_call` / `pet_select` | RPC `pet.select` `{profile?, slug}` | **Changes user state**: installs if needed, writes `display.pet.*`. |
| Uninstall | `pet_remove_call` / `pet_remove` | RPC `pet.remove` `{profile?, slug}` | **Destructive**: deletes the pet directory; turns the display off if it was active. |
| Rename | `pet_rename_call` / `pet_rename` | RPC `pet.rename` `{profile?, slug, name}` | **Changes user state**: renames the slug/directory, follows the active slug; reply `slug` is the new one. |
| Export | `pet_export_call` / `pet_export` | RPC `pet.export` `{profile?, slug}` | Reply `{ok, filename, zipBase64}` — base64 zip of the pet's files. |
| Thumbnail | `pet_thumb_call` / `pet_thumb` | RPC `pet.thumb` `{profile?, slug, url?}` | Reply `dataUri` (PNG data URL) for the picker. The handler fetches `url` only when it is Petdex-hosted; arbitrary URLs are never fetched. |
| Disable | `pet_disable_call` / `pet_disable` | RPC `pet.disable` `{profile?}` | **Changes user state**: `display.pet.enabled=false`. |
| Scale | `pet_scale_call` / `pet_scale` | RPC `pet.scale` `{profile?, scale}` | **Changes user state**: persists `display.pet.scale`, clamped by the backend; non-number → error 4004. |
| Cancel | `pet_cancel_call` / `pet_cancel` | RPC `pet.cancel` `{token}` | Idempotent, off the worker pool; unknown token is a no-op. |
| Generation status | `pet_generate_status_call` / `pet_generate_status` | RPC `pet.generate.status` `{}` | Local config read: `available` + `providers[{name,label,default}]`. |
| Generate drafts | `pet_generate_call` / `pet_generate` | RPC `pet.generate` `GenerateOptions` | **Spends model calls** (up to 4 image generations). `prompt` or `reference_image` required (4004 otherwise). `GENERATE_TIMEOUT` = 420 s. |
| Hatch | `pet_hatch_call` / `pet_hatch` | RPC `pet.hatch` `HatchOptions` | **Spends model calls + installs a pet** (one grounded call per animation row, with retries). Installed but not active. `HATCH_TIMEOUT` = 1 h. |

Notifications (`const` + payload struct + parser; `parse_pet_event` dispatches all three):

| Event | Payload | Parser |
|---|---|---|
| `PET_CHANGED` (`pet.changed`) | `PetChanged` — `pet.info.meta`-shaped, `{enabled:false}` when off | `parse_pet_changed` |
| `PET_GENERATE_PROGRESS` (`pet.generate.progress`) | `PetGenerateProgress` — token-only init frame, then `index` + `data_uri` per draft | `parse_pet_generate_progress` |
| `PET_HATCH_PROGRESS` (`pet.hatch.progress`) | `PetHatchProgress` — `event: row\|compose\|save`; row events also carry `state` + `done`/`total` (strings) | `parse_pet_hatch_progress` |

Response types (`PetSprite`, `PetInfo`, `PetInfoMeta`, `PetCells`, `PetGallery`/`PetGalleryEntry`,
`PetSlugResult`, `PetExport`, `PetThumb`, `PetOk`, `PetScale`, `PetProvider`,
`PetGenerateStatus`, `PetDraft`, `PetGenerate`, `PetHatch`, and the three payloads) are
lenient: `#[serde(default)]`, camelCase on the wire, so a missing or extra field never fails
a decode. `PetSprite` is shared by `pet.info` and `pet.hatch`'s `pet` preview.

## Safety

* Read-only/local: `pet.info`, `pet.info.meta`, `pet.cells`, `pet.generate.status`.
* Network: `pet.gallery` (Petdex manifest). `pet.select`/`pet.thumb` can also fetch from
  Petdex (install / thumbnail) when given a not-yet-installed slug.
* Spends model calls: `pet.generate`, `pet.hatch` — confirm with the user first.
* Changes user state: `pet.select`, `pet.remove`, `pet.rename`, `pet.disable`, `pet.scale`.
* **Never log** `spritesheet_base64`, `zip_base64`, `data_uri` or `PetDraft::data_uri` —
  they are multi-MB base64 blobs. Print lengths.

## Run live (private test backend)

`crates/hermes-core/tests/live_api_pets.rs` — 6 checks against `HERMES_DESKTOP_PORT=<port>`
(`scripts/live-test-backend.sh`): `pet.info`, `pet.info.meta`, `pet.cells`,
`pet.generate.status`, `pet.thumb` (unknown slug, no url → local miss) and `pet.cancel`
(random token → documented no-op). All six PASS on a fresh private backend (pet display
off, no providers configured).

## Not run live (and why)

| Endpoint | Why |
|---|---|
| `pet.generate`, `pet.hatch` | Spend model calls (image generation). |
| `pet.select`, `pet.remove`, `pet.rename`, `pet.disable`, `pet.scale` | Change persisted per-profile state; a throwaway pet cannot be created without model calls. |
| `pet.export` | Needs an installed pet; the reply is the pet's files as base64. |
| `pet.gallery` | Contacts the Petdex manifest (even `localOnly` prefetches). |
