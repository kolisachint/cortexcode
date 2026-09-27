---
name: canvas-design
description: How to make a canvas extension's page good — layout, styling, live state, and the dependency and token constraints the surface imposes. Read when building or editing a canvas; the /new-canvas brief points here.
disable-model-invocation: true
allowed-tools: read, write, edit, SearchCodebase
---

# Designing a canvas

A canvas is not a document. It is a small application: a page served over
loopback that a person operates while you drive the same state through typed
actions. Both of you are live on it at once, and that is the whole design
problem.

Read `../artifact-design/SKILL.md` for the fundamentals — treatment, the
color/type/layout plan you write before any markup, neutrals, spacing, the
generated-look list. All of it applies. This file covers only what is different
because the surface is a canvas, and where the two disagree, this file wins.

## What the catalog already tells you

Of the 23 extensions in GitHub's canvas catalog, 22 import nothing but
`@github/copilot-sdk` and `node:` builtins. The single exception ships a README
telling the user to `npm install` by hand. Zero dependencies is not a
restriction someone imposed on you; it is what working canvases actually do.

The flagship, `pr-artifact-explorer`, is a rich **read** surface the agent
navigates — its shared state is a cache and a route, and neither party co-edits
content. It sidesteps concurrent editing entirely. Treat that as evidence about
what is easy and what is not: a canvas that shows state well is worth far more
than one that lets both parties type into the same field.

## The page lives inside a template string

The HTML is a JavaScript template literal in `extension.mjs`, not a file. Two
consequences worth planning around rather than discovering:

- Every backtick and `${` in your CSS or markup needs escaping. A `grid-template`
  value or a JS snippet with a template literal inside it will break the outer
  string. Prefer plain quotes and avoid nesting template literals.
- There is no stylesheet to open in an editor. Keep the CSS in one clearly
  delimited chunk near the top of the served string so it stays findable, rather
  than scattering inline `style=` attributes.

If the page grows past a screenful of markup, serve it from a separate file in
the extension directory and read it at request time. The extension is already an
HTTP server; it can serve its own assets. That is the escape hatch from string
escaping, and it does not violate the no-dependency rule.

## No dependencies, and no build

`package.json` and `node_modules` are forbidden in the extension directory, so
there is nothing to install and nothing to bundle. Write plain CSS and plain
DOM. Modern CSS has custom properties, grid, and `clamp()`; a canvas that needs
a utility framework needs a clearer layout instead.

Do not reach for a CDN either. The page is served from `127.0.0.1`, so it works
with no network — a CDN script throws that away and turns an offline-capable
local tool into a broken one.

## You own the theme completely

The canvas protocol carries no theme, no palette, and no styling hook. Nothing
is inherited. A page that sets no colors gets browser defaults, which is why the
scaffold's placeholder looks like nothing.

So define the full palette as custom properties on `:root`, redefine them under
`@media (prefers-color-scheme: dark)`, and style everything through them. Set an
explicit `background` on `body`. This is the same discipline as any page, with
one simplification: there is no host stamping `data-theme`, so two states is the
whole problem.

## Design for two operators

The person changes state through the page. You change the same state through
`invoke_canvas_action`. The page has to be honest about that:

- **Render from server state, not from what the user just clicked.** After an
  action mutates state, the page must be able to show the new truth. Poll on a
  short interval, or push with SSE — both are a few lines with no dependency.
- **Make agent-reachable state visible.** If an action can add a note, the notes
  should be on screen. State only you can see is state the person cannot trust.
- **Do not build co-editing.** Concurrent edits to one field is the hard problem
  the reference canvas declined to solve. Prefer append, toggle, and select over
  a shared text buffer.
- **Show that something changed.** When state moves underneath the person, a
  brief highlight on the changed row beats a silent re-render.

## Actions are tool schemas, so they cost

Every action becomes an agent-callable tool while the instance is open, and its
name, description, and `inputSchema` are re-sent on every request for as long as
it stays open. Declare the actions the canvas actually needs and give each a
tight schema — not one per button.

Action results land in the model's context too. Return a summary and a count,
not the whole collection; where a list is genuinely useful, slice it and set a
flag saying you did. Measure with `hoocode --print-token-surface` while a canvas
is open.

## Reload replaces the URL

Reloading forks a new process and hands back a **new** URL; the tab the person
had open dies with the old one. Two design consequences:

- Keep meaningful state on the server side of the canvas, not in the page. A
  scroll position is fine to lose; a half-filled form is not.
- Give the person the new URL every time you reload, and say the old tab is
  dead. A page that silently stopped updating looks like a bug you caused.

## Before you call it done

- Open it and look, rather than reasoning about the markup.
- Check both color schemes.
- Resize narrow — a canvas is often a side window, not a full screen.
- Invoke each action and confirm the page reflects it without a manual refresh.
- Confirm the page still renders with the network off.
