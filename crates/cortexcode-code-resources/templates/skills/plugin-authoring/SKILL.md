---
name: plugin-authoring
description: How to author a portable, reusable hoocode plugin — when a capability is worth extracting, how to name and describe it so it triggers again, and what makes content portable across repos and machines. Use before calling ProposePlugin or UpdatePlugin, or when deciding whether a recipe you just completed is worth keeping.
allowed-tools: read, write, edit, SearchCodebase
---

# Authoring a plugin

This is the craft half of the plugin tools. `ProposePlugin` and `UpdatePlugin`
know how to *write* a plugin; this describes how to write a *good* one. Read it
before authoring, not after.

## When a capability is worth extracting

Extract when both are true:

- You completed a multi-step recipe you would plausibly repeat — or you repeated
  the same pattern twice in one session.
- `SearchPlugins` found nothing that already covers it.

Do not extract a one-off. A job that came up twice may just be a job that came up
twice; the test is whether the *shape* recurs, not whether the task did.

## Name and describe by the capability, not the occasion

The description is the only thing loaded on every turn, and it is the entire
basis on which the plugin is chosen later. Write it for the next situation, not
for the one that prompted it.

- Bad: `fix-flaky-auth-test` — names the incident. It will never trigger again.
- Good: `flaky-test-triage` — names the capability, with a description saying
  when to reach for it.

Say *when to use it* in the description, in the words someone would use when
they need it. A description that only says what the plugin is will not trigger.

## Portability

The plugin has to work in a repo you have never seen, on a machine you do not
control:

- No absolute paths, no machine-specific paths. Prefer relative paths and
  runtime discovery.
- No embedded secrets, tokens, or environment-specific values.
- No assumptions about the current repo unless that *is* the capability's point.
- State prerequisites in the body rather than assuming them.

The layout is the session's target platform. You never choose it.

## Shape

Put the whole plugin in one call — skills plus a hook go together, not in two
passes. The risk gate is computed from the content, so do not pre-classify:
read-only subagents, skills and commands go straight through; hooks, MCP
servers, or a subagent needing bash/write/edit/MCP or `tools: *` pause for human
confirmation.

Passive content activates immediately and is reversible with `UninstallPlugin`.
Announce what you created and why.

## Growing one you already authored

`UpdatePlugin` is additive: supply only the delta, and existing capabilities are
preserved (a matching name replaces just that one). It cannot remove anything —
that is `RemovePluginCapability`.

Hooks are the exception to "additive is safe". They have no name, so supplying a
changed command **adds a second hook alongside the old one** and both fire. To
change a hook, remove the old one first, then add the new one.

Adding a passive skill to an already-executable plugin does not re-prompt; only
executable *additions* trigger confirmation.

## Two things that are never yours to do

- Never grant a subagent a plugin-system tool (`InstallPlugin`, `ProposePlugin`,
  …). It is rejected, and the reason is that an agent that can install plugins
  for other agents is a self-propagation primitive.
- Never publish to a marketplace autonomously. Packaging is yours; publishing is
  a human action, because pushing executable code into a marketplace other
  agents install from unattended is a supply-chain compromise.
