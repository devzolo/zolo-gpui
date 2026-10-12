# Zolo GPUI

Native desktop UI for Zolo with GPUI 0.2.2 and Verniz components. Zolo owns
typed state, events, windows and component lifetimes; the Rust adapter owns the
native event loop and rendering.

## Install

Add the library to your application's zolo.toml:

```toml
[dependencies]
gpui = { git = "https://github.com/devzolo/zolo-gpui.git", rev = "main" }
```

Run `zolo install`, then `zolo dev main.zolo`. See the
[getting-started guide](docs/getting-started.md) for a complete counter,
compatible Zolo build, Rust/Cargo requirements and embedded deployment.
Keep the application's zolo.lock; pin a tested commit for a release.

## Components

| Area | API |
| --- | --- |
| Layout | Column, Row, Fragment, Spacer, Separator, Scroll, Split |
| Controls | Button, TextInput, TextArea, Checkbox, Switch, Select |
| Navigation | Tabs/Tab, List, Tree/TreeItem, Menu/MenuItem |
| Content | Text, RichText/TextSpan, Markdown, Code, Image, Icon, Canvas |
| Feedback | Badge, Progress, Dialog |
| Appearance | Style, Palette, light_palette |

Builders and Verniz tags use the same functions. Named `state<T>` cells retain
compatible values during development. Component/window contexts own callbacks,
subscriptions and asynchronous work. See the [desktop guide](docs/desktop.md)
for keys, controlled inputs, lifetimes, multiple windows and current limits.

## Develop locally

```powershell
cd examples
zolo install
zolo dev counter.zolo
```

The examples use a path dependency on this library. Keep a development session
open: Zolo UI saves reuse the loaded adapter and preserve compatible state.
Restart after Rust source or native dependency changes. Initial native
preparation can take several minutes and reports progress.

Run `zolo dev async_counter.zolo` for an asynchronous event example.

## Verify

From the library root:

```powershell
zolo run scripts/verify.zolo
zolo run scripts/verify-desktop.zolo
zolo run scripts/verify-dev.zolo
zolo run scripts/verify-core.zolo -- --library . --language-root "C:/path/to/zolo-lang" --reify --run --cli "C:/path/to/zolo.exe"
```

The window gates cover state, typed trees, Verniz children, controlled input,
Unicode editing, keyboard/modal focus, scrolling, split panels, dynamic windows
and owned asynchronous cleanup. Native/LLVM gates also execute a copied embedded
application without source compilers. The development-only native/Cargo.toml
supports direct Rust checks with `cargo check --locked`.

Windows x64 is the validated platform. Native GPUI windows cannot run in
standalone wasm. Physical OS IME composition, accessibility bridges and other
operating systems remain unverified. Lists/trees currently render all rows;
editor language services and native file pickers are supplied by applications.
