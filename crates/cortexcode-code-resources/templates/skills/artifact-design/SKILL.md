---
name: artifact-design
description: Build a self-contained HTML visual written to disk — page, report, dashboard, mockup, diagram, or poster. Load before the first line of markup or CSS whenever something has to look right, or a redesign is asked for. Not for app code the project's own design system already governs.
allowed-tools: read, write, edit, SearchCodebase
---

# Designing a visual

hoocode writes visuals as one self-contained `.html` file on disk. There is no
host, no CSP, and no gallery: fonts and libraries may come from a CDN or from
local files, and the page is yours to lay out however the subject demands. The
constraint that remains is the only one that ever mattered — it has to be good.

Not for a canvas extension. A canvas serves a live page from its own loopback
server, under constraints that contradict half of this file, and `/new-canvas`
points at `../canvas-design/SKILL.md` for it.

## First, read the request

Decide the treatment, not whether to design. Every visual gets real typographic
hierarchy, considered spacing, and a deliberate palette. What varies is how far
past that you go.

- **Utilitarian** — a report, a plan, a status page, a diagram. Most requests.
  Make it clean and well-composed. Skip the oversized hero. Keep flourishes few.
- **Editorial** — a landing page, a poster, something the user will show other
  people. Take a real point of view and one deliberate risk.

When unsure, build the well-composed version. A restrained page is never wrong;
an over-designed one sometimes is.

## Honor what already exists

Look before you invent. If the repo has a design system — a tokens file, a
theme, CSS variables, an existing stylesheet, a brand section in a project
context file — use it. Precedence is always:

1. What the user asked for, in their words
2. The project's existing system
3. Your own choices

Your choices fill gaps. They never override the first two.

## Write the plan before the markup

This is the part that does the work. Before any code, write down:

- **Color** — 4 to 6 named hex values. Name them by role (`ink`, `ground`,
  `accent`, `muted`), not by hue.
- **Type** — at least two faces with distinct jobs: one with character for
  display, one comfortable for body text, and a third for data or captions if
  the content needs it. Name the fallback stack for each.
- **Layout** — the structural idea in one or two sentences.

Then build from the plan and derive every color and type decision from it. A
page assembled without a plan reads as assembled without a plan.

## Fundamentals

**Ground it in the subject.** One concrete subject, one audience, one job for
the page. Distinctive choices come from the subject's own world — its materials,
its vocabulary, how people in it actually talk. Use real content throughout.
Never lorem, never placeholder rows.

**Type carries the page,** including when the page is not about type. Keep
running text near 65 characters wide. Set a scale and stay on it. Give headings
`text-wrap: balance`, give body text line-height room, give uppercase labels a
little letter-spacing. Always declare a real fallback stack — a silent fallback
is the most common way a good design ships looking wrong.

**Pick the neutrals.** A pure mid-grey reads as unconsidered. Bias the greys
slightly toward the accent hue and they read as chosen. Pure white and near-black
are fine grounds when the subject wants them; the point is that you decided.

**Let layout do the spacing.** Flex or grid with `gap`, not per-element margins
that collapse or double unpredictably. Wide content — tables, code blocks,
diagrams — gets its own `overflow-x: auto` container so the page body never
scrolls sideways. Use `font-variant-numeric: tabular-nums` wherever digits line
up in a column.

**Design both themes.** A local page sees only `prefers-color-scheme`, so this
is simpler here than on a hosted target: define the complete palette as custom
properties on `:root`, redefine only those properties inside
`@media (prefers-color-scheme: dark)`, and style every component through the
properties. Never give a color its only definition inside the media block — that
is how a page ends up rendering one theme's text on the other theme's ground.
Set an explicit `background` on `body`. Give the second theme the same attention
as the first rather than inverting it mechanically; check that the accent still
works on both grounds. A page that deliberately commits to one visual world may
stay single-theme, but then paint every color explicitly so it holds either way.

**Structure should encode something true.** Numbered markers, eyebrows,
dividers, and section labels are information, not decoration. Number things only
when the order actually matters to the reader.

**When it is a UI, not a document,** the craft shifts from typography to
information design. A dashboard is scanned, not read. Put the summary above the
detail. Encode state in form as well as in number — a pill, a chip, a severity
stripe — so what needs attention is visible at a glance. Semantic color (good,
warning, critical) is a separate system from your accent and does not count as
using it. Anything interactive should look interactive.

## Avoid the generated look

Machine-generated design keeps landing on the same handful of looks. When the
user has specified a direction, follow it exactly, including if it is one of
these. When nothing is specified, do not spend the freedom here:

- Warm cream ground, serif display face, terracotta accent
- Near-black ground with a single acid-green or vermilion pop
- Hairline rules and dense columns imitating a broadsheet
- Purple-to-blue gradient hero on white
- Inter or Space Grotesk chosen as the safe default
- Emoji as section markers
- Everything centered
- Uniform large corner radii on every surface
- A colored accent rail down the side of every card

## Words are design material

Write from the reader's side of the screen. Name things the way they would name
them, not the way the system is built. Active voice. A control says exactly what
it does, and the confirmation matches it. Errors say what went wrong and what to
do about it. Specific beats clever.

Give the page a real `<title>` — a short, specific noun phrase, not a category
label and not a name with an explainer bolted on after a dash.

## Libraries, and why the answer is usually none

A hosted page and a file on disk fail differently, and that decides this. A
hosted page is always viewed online, so a CDN dependency is free. A file gets
moved, attached to mail, and opened on a laptop in a tunnel — and there a
runtime dependency does not degrade, it collapses. Tailwind from a CDN with no
network is an unstyled document. React from a CDN with no network is a blank
one.

So the default is no runtime dependency at all. Write plain CSS; modern CSS has
custom properties, grid, `clamp()`, and container queries, and a page that needs
a utility framework to be laid out usually needs a clearer layout instead.

There is no build step here and nothing bundles this file, so a framework that
expects one — Vite, a JSX pipeline, anything importing bare module specifiers —
is not an option regardless.

When a library genuinely earns its place, and that is mostly charting or syntax
highlighting rather than layout:

- Inline it into the file if its licence permits, and the page stays whole.
- Otherwise pin an exact version, give the feature a readable fallback for when
  the script does not load, and tell the user the page needs network.

Webfonts are the one dependency that degrades gracefully, because a real
fallback stack keeps the page readable when the link fails. Use one, and always
declare the stack.

## Build cleanly

- Watch selector specificity. Type-level and element-level selectors fighting
  over the same padding is how spacing silently comes undone.
- Close every non-void element and double-quote every attribute.
- Give keyboard focus a visible state.
- Respect `prefers-reduced-motion`.
- For generative or decorative graphics, reach for Canvas or WebGL rather than
  hand-authoring long SVG path data.
- Before finishing, scan the stylesheet for any color declared only inside a
  media query.

## Delivering it

Write one self-contained `.html` file. Inline the page's own CSS and JS; embed
small assets as data URIs so the file survives being moved or sent to someone.

Put it where the user would expect it — alongside the data it visualizes, or in
the directory they named. Then give them the path as a markdown link with a
`file://` URL:

```
[tokens.html](file:///abs/path/to/tokens.html)
```

hoocode's markdown renderer turns that into an OSC 8 hyperlink wherever the
terminal supports one, so it is clickable in kitty, iTerm2, WezTerm and others,
and still readable as plain text everywhere else. Offer to open it rather than
opening it unasked.

Canvas is not a viewer for this. `/canvas` hosts canvas extensions — a directory
with an `extension.mjs` speaking the canvas JSON-RPC protocol — and `/canvas
open` takes an extension id, not a file path. A page you can hand someone is a
file; reach for `/new-canvas` only when the user wants a live surface the agent
can call typed actions on, which is a different and larger job than a visual.
