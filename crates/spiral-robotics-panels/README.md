# spiral-robotics-panels

`spiral-robotics-panels` is the reusable Dioxus window-management layer for Spiral Robotics
applications. The host registers panel content and supplies a canonical first
layout; the crate owns the serializable split tree, tabs, splitters, docking,
drag previews, keyboard operations, responsive panel switching, and common
chrome.

```rust
use spiral_robotics_panels::{LayoutNode, Panel, PanelLayout, PanelWorkspace, SplitAxis};

let panels = vec![
    Panel::new("controls", "Controls", "left", rsx! { Controls {} }),
    Panel::new("scene", "Scene", "main", rsx! { Scene {} }),
];

let layout = PanelLayout::new(LayoutNode::split(
    "root",
    SplitAxis::Horizontal,
    1.0 / 3.0,
    LayoutNode::tile("left", ["controls"]),
    LayoutNode::tile("main", ["scene"]),
));
```

Panel ids identify content; tile and split ids identify layout positions. All
three should remain stable across releases when persisted layouts need to
survive. A panel's `home` tile is used only when that panel is newly discovered
and absent from a restored layout.

At runtime:

- drag a tab to a tile center to attach it as a tab;
- drag to an edge to create a split;
- use the header actions to split right or down;
- use `Alt+Shift+Arrow` to split from the keyboard;
- use `Alt+Shift+PageUp/PageDown` to move across tile groups;
- drag or arrow-key a separator to resize it.

The included stylesheet uses semantic `--rp-*` variables with fallbacks to the
Studio's `--tone-*` tokens, so the mechanics stay reusable without creating a
second visual language.
