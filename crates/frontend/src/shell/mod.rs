//! Frontend providers and shell-level ownership boundaries.

pub(crate) mod activity;
pub(crate) mod frame;
mod gate;
pub(crate) mod notifications;
pub(crate) mod project;
pub(crate) mod routes;
mod sign_in;
pub(crate) mod state;
pub(crate) mod workspace;

use dioxus::prelude::*;

use self::routes::StudioRouter;

/// The Studio's stylesheet. DX links it from the page head, so the browser
/// loads it beside the bundle and paints nothing unstyled, `index.html`'s
/// startup page included. Nothing renders it; `#[used]` keeps it bundled.
#[used]
static STUDIO_STYLES: Asset = asset!(
    "/assets/tailwind.css",
    AssetOptions::css().with_static_head(true)
);

/// Render the routed shell; its layout owns application-wide providers.
#[component]
pub fn App() -> Element {
    rsx! {
        StudioRouter {}
    }
}
