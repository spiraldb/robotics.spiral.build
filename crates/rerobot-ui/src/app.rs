//! The studio shell: header, tabs, and the notice strip.

use dioxus::prelude::*;

/// The stylesheet the Dioxus CLI compiles from `tailwind.css`.
const TAILWIND: Asset = asset!("/assets/tailwind.css");

/// Inline so the tab icon needs no second request and no external host.
const FAVICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect width='32' height='32' rx='7' fill='%230284c7'/%3E%3Cpath d='M9 23V13l7-5 7 5v10' fill='none' stroke='white' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E";

use crate::cameras::CamerasTab;
use crate::control::ControlTab;
use crate::setup::SetupTab;
use crate::state::Studio;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tab {
    Setup,
    Control,
    Cameras,
}

impl Tab {
    const ALL: [Self; 3] = [Self::Setup, Self::Control, Self::Cameras];

    fn label(self) -> &'static str {
        match self {
            Self::Setup => "Setup",
            Self::Control => "Control",
            Self::Cameras => "Cameras",
        }
    }

    fn class(active: bool) -> &'static str {
        if active {
            "cursor-pointer border-b-2 border-sky-600 px-4 py-3 text-sm font-semibold text-sky-700 dark:text-sky-400"
        } else {
            "cursor-pointer border-b-2 border-transparent px-4 py-3 text-sm font-medium text-slate-500 hover:text-slate-800 dark:text-slate-400 dark:hover:text-slate-100"
        }
    }
}

#[component]
pub fn App() -> Element {
    // Publishes the shared state every other screen reads from context.
    Studio::use_studio();
    let mut tab = use_signal(|| Tab::Setup);

    rsx! {
        document::Stylesheet { href: TAILWIND }
        document::Title { "rerobot studio" }
        document::Link { rel: "icon", href: FAVICON }
        // Without this the browser renders form controls in the light theme
        // even when the page is dark.
        document::Meta { name: "color-scheme", content: "light dark" }

        div { class: "min-h-screen bg-slate-50 text-slate-900 dark:bg-slate-950 dark:text-slate-100",
            Header {}
            nav { class: "sticky top-0 z-10 border-b border-slate-200 bg-slate-50/90 backdrop-blur dark:border-slate-800 dark:bg-slate-950/90",
                div { class: "mx-auto flex max-w-7xl gap-1 px-4",
                    for entry in Tab::ALL {
                        button {
                            key: "{entry:?}",
                            r#type: "button",
                            class: Tab::class(tab() == entry),
                            onclick: move |_| tab.set(entry),
                            "{entry.label()}"
                        }
                    }
                }
            }
            Issues {}
            main { class: "mx-auto max-w-7xl px-4 py-6",
                match tab() {
                    Tab::Setup => rsx! { SetupTab {} },
                    Tab::Control => rsx! { ControlTab {} },
                    Tab::Cameras => rsx! { CamerasTab {} },
                }
            }
            NoticeStrip {}
        }
    }
}

#[component]
fn Header() -> Element {
    let studio = crate::state::studio();
    let snapshot = studio.snapshot.read();
    let live = studio.live;
    let mode = snapshot.mode.clone();
    let loop_hz = studio.telemetry.read().loop_hz;

    rsx! {
        header { class: "border-b border-slate-200 bg-white dark:border-slate-800 dark:bg-slate-900",
            div { class: "mx-auto flex max-w-7xl flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3",
                h1 { class: "text-lg font-semibold tracking-tight",
                    "rerobot "
                    span { class: "text-sky-600 dark:text-sky-400", "studio" }
                }
                span {
                    class: if mode.active { "chip bg-emerald-100 text-emerald-800 dark:bg-emerald-500/15 dark:text-emerald-300" } else { "chip bg-slate-100 text-slate-600 dark:bg-slate-800 dark:text-slate-300" },
                    "{mode.selection.label()}"
                }
                if snapshot.simulated {
                    span { class: "chip bg-amber-100 text-amber-800 dark:bg-amber-500/15 dark:text-amber-300",
                        "Simulated motors"
                    }
                }
                if let Some(error) = &mode.error {
                    span { class: "chip bg-rose-100 text-rose-800 dark:bg-rose-500/15 dark:text-rose-300",
                        "{error}"
                    }
                }
                div { class: "ml-auto flex items-center gap-3 text-xs text-slate-500 dark:text-slate-400",
                    if loop_hz > 0.0 {
                        span { class: "font-mono", "{loop_hz:.0} Hz" }
                    }
                    span { class: "flex items-center gap-1.5",
                        span {
                            class: if live() { "h-2 w-2 rounded-full bg-emerald-500" } else { "h-2 w-2 rounded-full bg-rose-500" },
                        }
                        {if live() { "connected" } else { "reconnecting" }}
                    }
                }
            }
        }
    }
}

/// Non-fatal problems the server wants the operator to know about.
#[component]
fn Issues() -> Element {
    let studio = crate::state::studio();
    let issues = studio.snapshot.read().issues.clone();
    if issues.is_empty() {
        return rsx! {};
    }

    rsx! {
        div { class: "mx-auto max-w-7xl px-4 pt-6",
            div { class: "rounded-lg border border-amber-300 bg-amber-50 px-4 py-3 text-sm text-amber-900 dark:border-amber-500/30 dark:bg-amber-500/10 dark:text-amber-200",
                p { class: "mb-1 font-medium", "Warnings" }
                ul { class: "list-inside list-disc space-y-0.5",
                    for issue in issues {
                        li { key: "{issue}", "{issue}" }
                    }
                }
            }
        }
    }
}

/// The outcome of the operator's last action.
#[component]
fn NoticeStrip() -> Element {
    let studio = crate::state::studio();
    let mut notice = studio.notice;
    let Some(current) = notice.read().clone() else {
        return rsx! {};
    };

    rsx! {
        div { class: "fixed inset-x-0 bottom-0 z-20 flex justify-center p-4",
            div {
                class: if current.failed { "flex max-w-2xl items-start gap-3 rounded-lg border border-rose-300 bg-rose-50 px-4 py-3 text-sm text-rose-900 shadow-lg dark:border-rose-500/30 dark:bg-rose-950 dark:text-rose-200" } else { "flex max-w-2xl items-start gap-3 rounded-lg border border-emerald-300 bg-emerald-50 px-4 py-3 text-sm text-emerald-900 shadow-lg dark:border-emerald-500/30 dark:bg-emerald-950 dark:text-emerald-200" },
                span { class: "flex-1", "{current.text}" }
                button {
                    r#type: "button",
                    class: "cursor-pointer font-medium underline underline-offset-2",
                    onclick: move |_| notice.set(None),
                    "Dismiss"
                }
            }
        }
    }
}
