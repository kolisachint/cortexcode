#!/usr/bin/env python3
"""
Regenerate the model catalog data files from the pinned hoocode build.

Loads hoocode's own built `models.generated.js` and `image-models.generated.js`
with node (so every TypeScript literal is read exactly) and writes them as flat
JSON arrays in hoocode's order (provider insertion order, then model order):

  crates/cortexcode-ai-models-catalog/data/models.json
  crates/cortexcode-ai-models-catalog/data/image-models.json
  crates/cortexcode-ai-models-catalog/data/pin.json   (the hoocode commit they came from)

Run after `migration/tui-parity/setup_hoocode.sh` (which builds the pin), and on
every pin bump:

  python3 scripts/convert_models_to_json.py [path/to/hoocode/packages/ai]
"""

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_AI_PACKAGE = ROOT / "target" / "hoocode-pin" / "packages" / "ai"
OUT_DIR = ROOT / "crates" / "cortexcode-ai-models-catalog" / "data"

DUMP = """
const [models, images] = await Promise.all([
  import(process.argv[1] + "/dist/models.generated.js"),
  import(process.argv[1] + "/dist/image-models.generated.js"),
]);
const flatten = (catalog) =>
  Object.values(catalog).flatMap((byId) => Object.values(byId));
process.stdout.write(JSON.stringify({
  models: flatten(models.MODELS),
  images: flatten(images.IMAGE_MODELS),
}));
"""


def main() -> int:
    package = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_AI_PACKAGE
    if not (package / "dist" / "models.generated.js").exists():
        print(f"{package}/dist is not built; run migration/tui-parity/setup_hoocode.sh", file=sys.stderr)
        return 1
    out = subprocess.run(
        ["node", "--input-type=module", "-e", DUMP, str(package.resolve())],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    data = json.loads(out)
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    commit = subprocess.run(
        ["git", "-C", str(package), "rev-parse", "HEAD"], check=True, capture_output=True, text=True
    ).stdout.strip()
    version = json.loads((package / "package.json").read_text())["version"]
    (OUT_DIR / "pin.json").write_text(
        json.dumps({"hoocodeVersion": version, "hoocodeCommit": commit}, indent=1) + "\n"
    )
    for name, key in (("models.json", "models"), ("image-models.json", "images")):
        path = OUT_DIR / name
        path.write_text(json.dumps(data[key], indent=1, ensure_ascii=False) + "\n")
        print(f"{path.relative_to(ROOT)}: {len(data[key])} entries")
    return 0


if __name__ == "__main__":
    sys.exit(main())
