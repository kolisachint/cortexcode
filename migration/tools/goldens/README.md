# Golden generators

Scripts that capture hoocode's output from the pinned build (`target/hoocode-pin`,
built by `migration/tui-parity/setup_hoocode.sh`) into Rust test fixtures.

Run with `migration/tools/goldens/run.sh <script> <output>`: the script is copied into
`packages/coding-agent/dist/` (so its relative imports resolve), run under
`COLORTERM=truecolor FORCE_COLOR=3` (a TTY-like chalk level, as the real app has), and
removed again.
