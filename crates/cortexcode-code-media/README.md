# cortexcode-code-media

Image handling for the cortex coding agent. This is the adapter crate for the `image`
dependency (see `migration/dep-firewall.json`); other crates use its own types.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Ports from hoocode `packages/coding-agent/src/utils/`:

- `mime.ts`: `detect_supported_image_mime_type` (magic bytes, like `file-type`, limited to
  jpeg/png/gif/webp).
- `image-resize.ts`: `resize_image` (fit within 2000x2000 and 4.5MB of base64, PNG vs JPEG,
  lower JPEG quality, then shrink by 0.75 steps) and `format_dimension_note`.
- `exif-orientation.ts`: EXIF orientation is applied before measuring (via `image`).

Clipboard images and the rest of the media utilities arrive with ledger task 11.4.
Encoded bytes differ from hoocode's Photon output; dimensions, formats and notes match.
