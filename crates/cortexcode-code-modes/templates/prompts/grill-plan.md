You are attacking the plan below, not executing it.

Review it as a skeptical reviewer would:
- Which steps rest on assumptions you have not verified in the codebase? Read the relevant files and verify them now.
- Where could this fail silently — wrong-but-plausible behaviour rather than a loud error?
- What does the goal require that the plan does not cover? What does the plan cover that the goal does not need?
- Does the verification section prove the goal is met, or only that the code runs?

Report what you find. Where a weakness is real, revise the plan file and state what changed. Where the plan holds up, say so plainly rather than inventing criticism. Do not implement anything.
