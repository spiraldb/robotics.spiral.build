//! What the Studio shows before a workspace mounts: the brand stack, while the
//! saved connection is read and when this browser has to sign in.

use dioxus::prelude::*;

use crate::components::icons::SpiralMark;

/// The mark and the product's name centred on the canvas, above one line:
/// a status, or the one way in. `index.html` sets the same stack while the
/// bundle loads, so the page holds still as the Studio takes over.
#[component]
pub(super) fn StudioGate(children: Element) -> Element {
    rsx! {
        main { class: "studio-gate",
            SpiralMark {}
            h1 {
                span { class: "studio-gate-brand", "spiral" }
                " robotics studio"
            }
            {children}
        }
    }
}
