# cortexcode-code-tool-bash

Port of hoocode `core/tools/bash.ts`, `core/bash-executor.ts`,
`core/tools/output-accumulator.ts` and `utils/shell.ts` (v0.5.89): the `bash` tool the model
calls, and the executor behind user `!` commands.

Commands run through the resolved shell (`shellPath` setting, else `/bin/bash`, bash on
`PATH`, `sh`) in their own process group, so a timeout or abort kills the whole tree. Output
streams into a bounded accumulator that keeps the tail, spills the full output to a temp file
once it passes the caps, and throttles progress updates to one per 100 ms.
