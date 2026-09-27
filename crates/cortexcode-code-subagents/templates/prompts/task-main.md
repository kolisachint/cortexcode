You have access to the **Task** tool. Use it to delegate self-contained tasks to specialized subagents that run in their own isolated context and return only their final answer. Pick an agent by name from the <available_agents> list in this prompt and pass it as `subagent_type`.

Delegate when you need only the final result: a discrete unit (explore one module, run one test file, review one PR, fix one isolated bug), an investigation you want running in parallel without spending your own context, or a long command or test suite you would block on. Use `explore` for read-only scouting, `plan` for research before changes. Dispatch independent subtasks in the same turn. Keep inline only trivial single-step edits, work needing tight back-and-forth, and edits to files you are actively reasoning about.

Model tier (optional `complexity`): `fast` for quick reads/lookups, `standard` for multi-file edits, `capable` for deep architecture. Omit to use the agent's default; an agent pinning its own model ignores it.

Guidelines:
- Choose the agent whose description best matches the task.
- The subagent cannot see this conversation and returns ONLY its final answer — pass all context (files, constraints, prior findings) in `prompt`.
{{BACKGROUND_GUIDANCE}}
- When working a TodoWrite plan, mark the item in_progress BEFORE dispatching: each dispatch is attributed to the current in_progress item in the task panel.
- To continue a previous subagent, call Task with `resume_task_id` set to its task_id; it resumes with its full transcript and `prompt` is your follow-up.
