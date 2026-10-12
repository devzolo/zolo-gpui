# Desktop toolkit

GPUI owns the native event loop. Zolo owns typed state, component lifetimes,
events, background tasks and application services. The source package includes
the general desktop components and their Rust rendering adapter. Start with
the [getting-started guide](getting-started.md).

## State, components and lifetimes

```zolo
use gpui::{state, Node, Component, Button, Text}
struct Model { count: int = 0, labels: [str] = [] }
let model = state("model", Model.new())
fn Counter() -> Node {
    <Button on_click={|| model.update(|value| {
        value.count += 1
        return value
    })}>
        <Text value={model.value.count} />
    </Button>
}
fn Desktop() -> Node { Component("counter", Counter) }
```

Reading a state cell records the current component/window dependency. Assigning
`.value` or using `.update` invalidates dependent components and their cached
parents. Use `.update` when editing nested models or collections. Subscription
callbacks run under their owning context; `.dispose` explicitly removes one.
Render-owned subscriptions are disposed on rerender or unmount.

Top-level keys share state deliberately. Keys created during rendering are
scoped to application, window and component identity. Keep component IDs and
state keys stable across edits. `state<T>` uses the language's general
`@reify(T)` type witness: incompatible nested/generic schemas reject before
returning an old cell. Use a new key or restart for a schema migration.

`List(id: "files", items: files, key: |file| file.path, render: |file| FileRow(file))`
requires a nonempty ID and unique, nonempty row keys. Each row owns a component
scope, so local state and tasks follow the logical item when rows are reordered.
Removing a row disposes its subscriptions and cancels its owned tasks. A rejected
view replacement also removes named state cells first created by that candidate.
Its cancelled tasks retain access to those detached cells until their own cleanup
has physically finished. A later candidate can immediately create the same keys
with another schema; it cannot inherit the rejected candidate's controller.

`context()` captures the current component/frame lifetime. `Context.run`
starts work under that lifetime; `is_active()` checks it without throwing.
`Window.context()` is an explicit durable window lifetime. Native App/Window
borrows never cross these capabilities or an `await`.


## Windows

```zolo
use gpui::{application, WindowOptions, Text}
let app = application("desktop")
let main = app.window("main", || Text("Main"),
    WindowOptions.new(title: "Workspace", width: 1200.0, height: 800.0))
let tools = app.window("tools", || Text("Tools"))
app.run()
```

`App.window` can also run inside events. Window task groups are siblings under
the application, so closing the opening window does not cancel a new one.
`Window` exposes close, title, resize, focus, notify, context and is_open.
`App.quit` closes all windows. Native operations validate allocation identity;
constructing an empty lookalike handle does not grant access.

During development, replacing a view is staged. Zolo rendering and native
tree validation must succeed before the old frame is released. Failure keeps
the previous tree, handlers, subscriptions and tasks. `Window.last_error`
contains the rejected replacement error. Arbitrary application side effects
performed during rendering cannot be rolled back; keep rendering declarative.


## Components

| Area | API |
| --- | --- |
| Layout | Column, Row, Fragment, Spacer, Separator, Scroll, Split |
| Controls | Button, TextInput, TextArea, Checkbox, Switch, Select |
| Navigation | Tabs/Tab, List, Tree/TreeItem, Menu/MenuItem |
| Content | Text, RichText/TextSpan, Markdown, Code, Image, Icon, Canvas |
| Feedback | Badge, Progress, Dialog |
| Appearance | Style, Palette, light_palette |

Builders and Verniz tags call the same typed functions. Node's general
`@naming` return policy accepts component names without disabling other lints.
Callbacks may return an ordinary value or a Future; the event driver observes
async failures and drains cleanup.

Columns size to their content. `Style.grow` distributes remaining space along
the parent's layout axis; `Style.shrink` controls compression when space is
limited. Their defaults are `0.0` and `1.0`. Use `grow: 1.0` on a panel that
should expand, including reusable root components inside another container.
Automatic panel filling respects explicit width/height. Explicit `grow` or
`shrink` can expand or compress that size; use `shrink: 0.0` when a viewport
must retain its height. Scroll keeps a bounded viewport
and the natural heights of its content, so long cards scroll instead of being
compressed over their neighbors.

Text controls support Unicode selection, clipboard actions, undo/redo,
multiline wrapping, readonly/disabled states and IME composition. A host update
acknowledges the exact processed edit sequence; an older update cannot erase
newer queued typing. Native editing chords precede application shortcuts.
Keyboard handlers route to the nearest focused ancestor; modal dialogs limit
navigation and shortcuts to their own contents, then restore focus on close.

`Split` has two panes and reports a controlled ratio through `on_resize`.
Markdown is parsed into rich text and code blocks. Links call `on_link`; the
host chooses navigation. Images/icons use local paths. Canvas supports rect,
ellipse, line and text primitives in logical pixels.


Authored IDs may contain paths, percent signs and tildes. Each identity segment
is escaped independently; authored names never alias nested components or
positional children. `Window.focus` accepts a unique authored ID or a complete
encoded path. Complete paths use `%2F`, `%25`, `%7E` inside authored segments,
and `~n` for unnamed child positions. Prefer stable authored IDs for focus.


## Persistent development

Use `zolo dev counter.zolo` from `examples/` and keep the session open.
Entry and imported Zolo component saves reuse the admitted native adapter and
dependency analysis. A successful module update invalidates retained root and
keyed component functions, so their next render uses the new implementation.
New named props and defaults are available to the next entry compilation.

Compatible typed state survives module and entry updates. Reification retains
the complete selected imported contract during live compilation; changing a
cell's nested schema rejects that candidate with a diagnostic. Syntax and
execution errors retain the prior valid view and handlers. Each replay in a
batch has its own publication transaction. State mutations or external effects
made by application code are not transactional; keep view construction
declarative. New native imports or edits to admitted Rust sources request a
session restart.

`pwsh -File scripts/verify-dev.ps1` measures source-write to the Zolo render
callback in an actual owned window. It also checks state, window identity,
rejected schemas, error recovery, imported props and rapid successive saves.
This receipt does not measure the operating system's completed screen paint.

The isolated Windows x64 gate on 2026-10-11 measured 423.0 ms for the first
entry save, 248.3 ms for an imported module, 578.7 ms for a complete typed
component change, 282.5 ms for recovery and 282.0 ms for rapid consecutive
saves. Compatible state, the same window and handlers survived; incompatible
schemas were rejected with the original diagnostic. Session startup took
59.0 seconds once, including cached dependency verification. UI saves did not
repeat native preparation or run Cargo. These local measurements are not
latency guarantees.


## Current limits

Windows x64 is the integration-test platform. Native GPUI windows are unavailable
in standalone wasm. The adapter uses pinned GPUI 0.2.2.
Unicode keyboard input and native editing are verified; physical OS IME
composition has not been exercised by the actual-window gates.

From the library root, `scripts/verify-desktop.ps1`
exercise actual owned windows with the installed CLI (`-Backend native` or
`-Backend llvm` also checks a copied executable without source compilers).
`scripts/verify-core.cjs` retains the private state/controller regressions in
`tests/pure`. It requires Rust and the Zolo source checkout:

```powershell
node scripts/verify-core.cjs --library . --language-root <Zolo checkout> --reify --run --cli <fresh CLI>
```

That isolated fixture stubs native functions and adjusts generated top-level
locals for Lua's chunk limit. Its results complement the actual window gates.

`node scripts/verify-normal-host-drain.cjs --cli <fresh CLI>` checks normal host
return, host errors and cleanup errors on VM/native/LLVM. Its generated host is
an explicit stub; async cleanup and context lifetime use the actual library.
The platform may exit before its next close callback. `App.run` still drains
every owned group before returning, and preserves the original host error.

`node scripts/verify-entry-reload.cjs --cli <fresh CLI>` checks rejected entry
replays, mixed module/entry batches, multiple entries and retired window cleanup.
The renderer uses the generic `std::ui::poll_updates(on_begin, on_end)` callbacks:
each execution has a separate snapshot and success result. An aggregate update
notification cannot publish a failed entry's pending view, title or new window.

Code views/editor text are monospaced; syntax diagnostics, completion and syntax
coloring require an editor service supplied by the application. Lists/trees
currently render all rows. Menus are inline compositions. Native OS file pickers
are injected callbacks. Accessibility platform bridges and other operating
systems have not been validated by the Windows gates.

TaskGroup drives cooperative code. Blocking CPU/IO work still blocks its thread.
Synchronous scope/parallel joins and streaming await deadlines remain explicitly
unsupported inside the foreground group. DNS cancellation can retain a pending
physical receipt until the system resolver returns. Async Rust source functions
use the admitted physical-worker adapter; other foreign providers must implement
the same generic cancellation/drain contract before joining a foreground group.
