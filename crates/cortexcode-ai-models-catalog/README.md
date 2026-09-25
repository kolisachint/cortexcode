# cortexcode-ai-models-catalog

The model catalog as data: hoocode's `models.generated.ts` and `image-models.generated.ts`
at the pinned commit, flattened to JSON arrays in hoocode's order. Lookup logic lives in
`cortexcode-ai-models` (LLM models) and `cortexcode-ai-images` (image models).

Regenerate on every hoocode pin bump (after `migration/tui-parity/setup_hoocode.sh`):

    python3 scripts/convert_models_to_json.py

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.
