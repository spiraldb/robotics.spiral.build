//! Serving the compiled web UI.
//!
//! The UI is a WebAssembly bundle built separately by the Dioxus CLI, so the
//! server may legitimately start before it exists. When it does not, every
//! non-API route explains how to build it rather than returning a bare 404.

use std::path::{Path, PathBuf};

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use tower_http::services::{ServeDir, ServeFile};

/// Where the Dioxus CLI leaves a web bundle, relative to the UI crate root.
///
/// `dx build` writes `target/dx/<crate>/<profile>/web/public`. The release
/// bundle is preferred so a stale debug build never shadows it.
const DX_BUNDLES: [&str; 2] = [
    "target/dx/rerobot-ui/release/web/public",
    "target/dx/rerobot-ui/debug/web/public",
];

/// Locate the built UI, preferring an explicit path.
///
/// Candidates cover the two ways the studio actually runs: from a checkout
/// during development, and from an installed binary with the bundle beside it.
pub fn resolve_ui_dir(explicit: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(explicit) = explicit {
        return is_built(&explicit).then_some(explicit);
    }

    let from_checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rerobot-ui");
    let from_cwd = PathBuf::from("crates/rerobot-ui");
    let mut candidates = Vec::new();
    for root in [&from_checkout, &from_cwd] {
        candidates.extend(DX_BUNDLES.iter().map(|bundle| root.join(bundle)));
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        candidates.push(directory.join("ui"));
    }

    candidates.into_iter().find(|candidate| is_built(candidate))
}

/// A directory counts as a built UI only if it can actually be served.
fn is_built(directory: &Path) -> bool {
    directory.join("index.html").is_file()
}

/// Routes serving the UI, or an explanatory page when it has not been built.
pub fn ui_router(directory: Option<&Path>) -> Router {
    match directory {
        Some(directory) => {
            let index = directory.join("index.html");
            // Unknown paths fall back to the single page, so a refresh on any
            // in-app route still loads the app.
            Router::new().fallback_service(ServeDir::new(directory).fallback(ServeFile::new(index)))
        }
        None => Router::new().fallback(missing_ui),
    }
}

async fn missing_ui() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::CACHE_CONTROL, "no-store")],
        Html(MISSING_UI_PAGE),
    )
        .into_response()
}

const MISSING_UI_PAGE: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>rerobot studio &mdash; UI not built</title>
<style>
  body { font: 16px/1.6 system-ui, sans-serif; margin: 4rem auto; max-width: 42rem; padding: 0 1.5rem; }
  code, pre { background: #f4f4f5; border-radius: 6px; }
  code { padding: 0.15em 0.4em; }
  pre { padding: 1rem; overflow-x: auto; }
</style>
<h1>The web UI has not been built yet</h1>
<p>The studio server is running and its API is live, but the compiled
WebAssembly bundle was not found.</p>
<pre><code>nix develop path:.
cd crates/rerobot-ui &amp;&amp; dx build --release</code></pre>
<p>Then reload this page. Pass <code>--ui-dir PATH</code> if the bundle lives
somewhere else.</p>
<p>The API is available meanwhile, starting with <a href="/api/state">/api/state</a>.</p>
"#;

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn an_explicit_directory_is_used_only_when_it_holds_a_page() {
        let directory = TempDir::new().expect("temporary directory");

        assert_eq!(resolve_ui_dir(Some(directory.path().to_path_buf())), None);

        fs::write(directory.path().join("index.html"), "<h1>ui</h1>").unwrap();
        assert_eq!(
            resolve_ui_dir(Some(directory.path().to_path_buf())),
            Some(directory.path().to_path_buf())
        );
    }

    #[test]
    fn a_missing_explicit_directory_does_not_fall_back_to_a_guess() {
        // Silently serving a different build than the one asked for would be
        // worse than saying the bundle is missing.
        assert_eq!(
            resolve_ui_dir(Some(PathBuf::from("/nonexistent/rerobot-ui"))),
            None
        );
    }
}
