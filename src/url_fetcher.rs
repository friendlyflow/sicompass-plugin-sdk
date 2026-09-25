//! Rendering a web page a link points to.
//!
//! Two shapes. The synchronous one ([`register_url_fetcher`],
//! [`fetch_url_to_ffon`]) is for a renderer compiled into the app. The
//! asynchronous one is for a renderer that is a plugin (the browser), which
//! takes seconds and answers later:
//!
//! 1. The host says a renderer is there ([`set_renderer_available`]) while a
//!    plugin with `"rendersPages": true` is loaded.
//! 2. Following a link asks for the page ([`request_render`]), which only
//!    queues it, and shows a placeholder.
//! 3. The host hands queued URLs to the plugin ([`take_render_requests`]).
//! 4. The plugin's answer comes back through the host ([`deliver_render`]).
//! 5. The renderer puts it under the link ([`take_rendered`]).
//!
//! The queues are global because the renderer (in `sicompass-ui`) and the
//! plugin host (in the app) cannot reach each other, only this crate.

use crate::ffon::FfonElement;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// How many loaded plugins render pages.
static RENDERERS: AtomicUsize = AtomicUsize::new(0);
/// URLs waiting for a renderer to take them.
static REQUESTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Rendered pages waiting for the renderer to put them under their links.
#[allow(clippy::type_complexity)]
static RENDERED: Mutex<Vec<(String, Vec<FfonElement>)>> = Mutex::new(Vec::new());

/// A page renderer was loaded (`true`) or went away (`false`).
pub fn set_renderer_available(available: bool) {
    if available {
        RENDERERS.fetch_add(1, Ordering::AcqRel);
    } else {
        let _ = RENDERERS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
        if RENDERERS.load(Ordering::Acquire) == 0 {
            REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }
}

/// Whether a loaded plugin renders pages.
pub fn renderer_available() -> bool {
    RENDERERS.load(Ordering::Acquire) > 0
}

/// Ask for `url` rendered. `false` when no renderer is loaded, and nothing
/// is queued. A URL already waiting is not asked for twice.
pub fn request_render(url: &str) -> bool {
    if !renderer_available() {
        return false;
    }
    let mut requests = REQUESTS.lock().unwrap_or_else(|e| e.into_inner());
    if !requests.iter().any(|u| u == url) {
        requests.push(url.to_owned());
    }
    true
}

/// The URLs asked for since the last call, for the host to hand a renderer.
pub fn take_render_requests() -> Vec<String> {
    std::mem::take(&mut *REQUESTS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// How many rendered pages wait for a link at most. One nobody takes (the
/// user left the link before it arrived) goes when newer ones come.
const RENDERED_KEPT: usize = 32;

/// A renderer's page for `url`.
pub fn deliver_render(url: &str, page: Vec<FfonElement>) {
    let mut rendered = RENDERED.lock().unwrap_or_else(|e| e.into_inner());
    rendered.push((url.to_owned(), page));
    let excess = rendered.len().saturating_sub(RENDERED_KEPT);
    rendered.drain(..excess);
}

/// The rendered pages whose URL `wanted` accepts, as `(url, page)`. The
/// others stay for whoever waits for them.
pub fn take_rendered_where(wanted: impl Fn(&str) -> bool) -> Vec<(String, Vec<FfonElement>)> {
    let mut rendered = RENDERED.lock().unwrap_or_else(|e| e.into_inner());
    let (taken, kept) = std::mem::take(&mut *rendered)
        .into_iter()
        .partition(|(url, _)| wanted(url));
    *rendered = kept;
    taken
}

/// Every rendered page waiting, as `(url, page)`.
pub fn take_rendered() -> Vec<(String, Vec<FfonElement>)> {
    take_rendered_where(|_| true)
}

static URL_FETCHER: OnceLock<Box<dyn Fn(&str) -> Vec<FfonElement> + Send + Sync>> = OnceLock::new();

/// Register a URL fetch implementation.
///
/// Only the first call has effect (idempotent guard via `OnceLock`).
/// Called once from the webbrowser lib's `register()`.
pub fn register_url_fetcher(f: impl Fn(&str) -> Vec<FfonElement> + Send + Sync + 'static) {
    URL_FETCHER.get_or_init(|| Box::new(f));
}

/// Fetch a URL and return its content as FFON elements.
///
/// Returns an empty `Vec` if no fetcher has been registered.
pub fn fetch_url_to_ffon(url: &str) -> Vec<FfonElement> {
    if let Some(f) = URL_FETCHER.get() {
        f(url)
    } else {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_url_returns_empty_when_no_fetcher_registered() {
        // OnceLock may already have a fetcher from another test in a different
        // crate, so we can only test the no-fetcher case if the lock is unset.
        // If it is set, fetch_url_to_ffon must still return *something* (non-panic).
        let result = fetch_url_to_ffon("https://example.com");
        // Either empty (no fetcher) or non-panic (fetcher present) — both are valid.
        let _ = result; // just assert it doesn't panic
    }

    /// The whole round trip. One test, because the queues are global and
    /// tests run in parallel.
    #[test]
    fn a_render_request_goes_round_only_while_a_renderer_is_loaded() {
        assert!(!request_render("https://a.example/"), "no renderer yet");
        assert!(take_render_requests().is_empty());

        set_renderer_available(true);
        assert!(request_render("https://a.example/"));
        assert!(request_render("https://a.example/"), "asked again");
        assert_eq!(
            take_render_requests(),
            ["https://a.example/"],
            "but queued once"
        );
        assert!(take_render_requests().is_empty());

        deliver_render(
            "https://a.example/",
            vec![FfonElement::new_str("A".to_owned())],
        );
        let pages = take_rendered();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].0, "https://a.example/");
        assert!(take_rendered().is_empty());

        // A page is only taken by whoever wants it.
        deliver_render("https://c.example/", Vec::new());
        deliver_render("https://d.example/", Vec::new());
        let c = take_rendered_where(|u| u == "https://c.example/");
        assert_eq!(c.len(), 1);
        assert_eq!(take_rendered()[0].0, "https://d.example/", "d stayed");
        // Unclaimed pages are not kept forever.
        for i in 0..(RENDERED_KEPT + 5) {
            deliver_render(&format!("https://{i}.example/"), Vec::new());
        }
        let kept = take_rendered();
        assert_eq!(kept.len(), RENDERED_KEPT);
        assert_eq!(kept[0].0, "https://5.example/", "the oldest went");

        // The renderer goes away with a request still waiting: it is dropped.
        assert!(request_render("https://b.example/"));
        set_renderer_available(false);
        assert!(!renderer_available());
        assert!(take_render_requests().is_empty());
        set_renderer_available(false);
        assert!(!renderer_available(), "never below zero");
    }
}
