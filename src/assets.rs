//! Provider-scoped assets, named by an `asset:<provider>/<name>` URI.
//!
//! # Why a URI and not bytes
//!
//! Every place an asset is named is already a string: [`crate::Provider::dashboard_image_path`]
//! returns `Option<&str>`, `<image>…</image>` and `<link>…</link>` carry strings
//! inside FFON text, and the host's texture cache is keyed by that same string.
//! Handing bytes across those interfaces would mean changing all of them, and
//! would break the cache key (bytes are not a stable identity for "the same
//! picture"). So an asset keeps being *named*; only the resolution changes.
//!
//! The host already treats an `<image>` value as URI-ish, branching on an
//! `http://` / `https://` prefix before falling back to a filesystem path. This
//! adds one more scheme to that space.
//!
//! # Why the assets themselves live in the owning crate
//!
//! A built-in provider `include_bytes!`s its files out of its own `assets/`
//! directory and publishes them here in `register()`:
//!
//! ```ignore
//! const TEXTURE: &[u8] = include_bytes!("../assets/texture.jpg");
//!
//! pub fn register() {
//!     sicompass_sdk::assets::register_bytes("tutorial", "texture.jpg", TEXTURE);
//!     // …then the factory, so a provider built by the next line already resolves.
//! }
//! ```
//!
//! Nothing is read from a runtime asset tree, so nothing has to be listed in the
//! archive `include`, the cargo-packager resources, the rpm asset globs and the
//! MSI components. Those five hand-maintained lists are what silently shipped
//! release archives with no assets at all, once, for a long while.
//!
//! A WASM plugin cannot compile anything into the host binary, so the host
//! registers a *resolver* for it instead, reading `<plugin_dir>/assets/<name>`
//! under the same confinement as every other guest-supplied path. Same URI shape,
//! same call site, different byte source.

// ---------------------------------------------------------------------------
// Portable — the URI vocabulary
// ---------------------------------------------------------------------------
//
// A guest builds with `default-features = false` and still has to *name* its
// assets, so the scheme lives on the portable side. Only the registry below is
// host-only.

/// Scheme marking a provider-scoped asset rather than a path or a URL.
pub const SCHEME: &str = "asset:";

/// Build the URI naming `name` within `provider`'s asset set.
///
/// `provider` should match the id the provider is registered under, and should
/// avoid whitespace: the result ends up inside FFON text where it is read back by
/// a plain prefix test.
pub fn uri(provider: &str, name: &str) -> String {
    format!("{SCHEME}{provider}/{name}")
}

/// True when `s` names a registered asset. Cheap enough for a per-frame path.
pub fn is_uri(s: &str) -> bool {
    s.starts_with(SCHEME)
}

/// Split an asset URI into `(provider, name)`.
///
/// The split is on the *first* `/`, so a name may itself contain slashes:
/// `asset:p/sub/dir/x` is `("p", "sub/dir/x")`. Returns `None` unless `s` carries
/// the scheme and at least one `/` after it, i.e. both halves exist.
pub fn parse_uri(s: &str) -> Option<(&str, &str)> {
    let rest = s.strip_prefix(SCHEME)?;
    let (provider, name) = rest.split_once('/')?;
    if provider.is_empty() || name.is_empty() {
        return None;
    }
    Some((provider, name))
}

// ---------------------------------------------------------------------------
// Host-only — the registry
// ---------------------------------------------------------------------------

#[cfg(feature = "host")]
mod registry {
    use super::parse_uri;
    use std::borrow::Cow;
    use std::collections::HashMap;
    use std::sync::{OnceLock, RwLock};

    /// Byte source for one provider's whole asset set. `None` means "no such
    /// asset", which the caller treats exactly as it treats a missing file.
    pub type AssetResolver = Box<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync>;

    #[derive(Default)]
    struct Registry {
        /// Full URI -> compiled-in bytes. Built-in providers.
        bytes: HashMap<String, &'static [u8]>,
        /// Provider id -> fallible byte source. WASM plugins.
        resolvers: HashMap<String, AssetResolver>,
    }

    /// `RwLock`, not the `Mutex` the manifest and factory registries use: those are
    /// read once at startup and clone a snapshot out, while this one is read on the
    /// host's texture-cache-miss path. Registration is still a startup-time,
    /// write-once affair.
    static REGISTRY: OnceLock<RwLock<Registry>> = OnceLock::new();

    fn registry() -> &'static RwLock<Registry> {
        REGISTRY.get_or_init(|| RwLock::new(Registry::default()))
    }

    /// Publish one compiled-in asset. Call from the owning crate's `register()`,
    /// before registering the provider factory, so a provider constructed
    /// immediately afterwards already resolves.
    ///
    /// Overwrites a same-key registration, so calling `register()` twice (which
    /// tests do) is harmless.
    pub fn register_bytes(provider: &str, name: &str, bytes: &'static [u8]) {
        if let Ok(mut reg) = registry().write() {
            reg.bytes.insert(super::uri(provider, name), bytes);
        }
    }

    /// Publish a byte source for a whole provider — how the host serves a WASM
    /// plugin's `<plugin_dir>/assets/` directory. Replaces any previous resolver
    /// for `provider`.
    ///
    /// The closure runs while the registry's read lock is held, so it must not
    /// call back into this module. Reading a file is fine; registering is not.
    pub fn register_resolver(provider: &str, resolver: AssetResolver) {
        if let Ok(mut reg) = registry().write() {
            reg.resolvers.insert(provider.to_owned(), resolver);
        }
    }

    /// Resolve `asset:<provider>/<name>` to bytes.
    ///
    /// A compiled-in registration wins over a resolver for the same URI: a
    /// built-in's bytes are in the binary and cannot go missing, while a
    /// resolver's file can, and preferring the certain one keeps a plugin from
    /// shadowing a built-in asset by name.
    ///
    /// `None` for an unknown provider, an unknown name, or a string that is not an
    /// asset URI at all.
    pub fn resolve(uri: &str) -> Option<Cow<'static, [u8]>> {
        let (provider, name) = parse_uri(uri)?;
        let reg = registry().read().ok()?;
        if let Some(bytes) = reg.bytes.get(uri) {
            return Some(Cow::Borrowed(bytes));
        }
        reg.resolvers.get(provider)?(name).map(Cow::Owned)
    }

    /// [`resolve`] plus a UTF-8 check, for text assets (JSON, FFON source).
    pub fn resolve_str(uri: &str) -> Option<Cow<'static, str>> {
        match resolve(uri)? {
            Cow::Borrowed(b) => std::str::from_utf8(b).ok().map(Cow::Borrowed),
            Cow::Owned(v) => String::from_utf8(v).ok().map(Cow::Owned),
        }
    }

    /// Every compiled-in asset URI, sorted. Drives `sicompass --check`, which
    /// resolves each one so a provider naming an asset it never registered is
    /// reported rather than showing up later as a silently missing image.
    ///
    /// Resolver-backed providers are absent by construction: a closure is not a
    /// listing, so a WASM plugin's assets cannot be enumerated here.
    pub fn registered_uris() -> Vec<String> {
        let Ok(reg) = registry().read() else {
            return Vec::new();
        };
        let mut out: Vec<String> = reg.bytes.keys().cloned().collect();
        out.sort();
        out
    }
}

#[cfg(feature = "host")]
pub use registry::{
    AssetResolver, register_bytes, register_resolver, registered_uris, resolve, resolve_str,
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_and_parse_uri_are_inverses() {
        let u = uri("tutorial", "texture.jpg");
        assert_eq!(u, "asset:tutorial/texture.jpg");
        assert!(is_uri(&u));
        assert_eq!(parse_uri(&u), Some(("tutorial", "texture.jpg")));
    }

    #[test]
    fn a_name_may_contain_slashes() {
        // The split is on the first `/` only, so a provider can nest its assets.
        assert_eq!(
            parse_uri("asset:p/sub/dir/x.png"),
            Some(("p", "sub/dir/x.png"))
        );
    }

    #[test]
    fn a_path_or_a_url_is_not_an_asset_uri() {
        for s in ["/tmp/x.png", "https://example.com/y.png", "texture.jpg"] {
            assert!(!is_uri(s), "{s}");
            assert_eq!(parse_uri(s), None, "{s}");
        }
    }

    #[test]
    fn a_uri_missing_either_half_does_not_parse() {
        // `is_uri` is a prefix test and says yes to all of these; `parse_uri` is
        // what decides they name nothing.
        for s in ["asset:", "asset:noslash", "asset:/name", "asset:provider/"] {
            assert_eq!(parse_uri(s), None, "{s}");
        }
    }

    // The registry is process-global, so every test below uses its own provider
    // id. Same discipline as the factory-registry tests in `provider.rs`.

    #[cfg(feature = "host")]
    #[test]
    fn a_registered_asset_resolves_and_an_unknown_one_does_not() {
        register_bytes("__test_static", "a.bin", b"hello");
        assert_eq!(
            resolve("asset:__test_static/a.bin").as_deref(),
            Some(&b"hello"[..])
        );
        assert!(resolve("asset:__test_static/missing.bin").is_none());
        assert!(resolve("asset:__test_unregistered/a.bin").is_none());
        assert!(resolve("/tmp/not-a-uri").is_none());
    }

    #[cfg(feature = "host")]
    #[test]
    fn a_resolver_serves_a_whole_provider() {
        register_resolver(
            "__test_resolver",
            Box::new(|name| (name == "known.txt").then(|| b"from-resolver".to_vec())),
        );
        assert_eq!(
            resolve("asset:__test_resolver/known.txt").as_deref(),
            Some(&b"from-resolver"[..])
        );
        assert!(resolve("asset:__test_resolver/other.txt").is_none());
    }

    #[cfg(feature = "host")]
    #[test]
    fn a_compiled_in_asset_wins_over_a_resolver() {
        register_resolver("__test_both", Box::new(|_| Some(b"resolver".to_vec())));
        register_bytes("__test_both", "x.bin", b"static");
        assert_eq!(
            resolve("asset:__test_both/x.bin").as_deref(),
            Some(&b"static"[..])
        );
        // A name the static half does not carry still reaches the resolver.
        assert_eq!(
            resolve("asset:__test_both/y.bin").as_deref(),
            Some(&b"resolver"[..])
        );
    }

    #[cfg(feature = "host")]
    #[test]
    fn resolve_str_refuses_bytes_that_are_not_utf8() {
        register_bytes("__test_utf8", "good.txt", b"text");
        register_bytes("__test_utf8", "bad.bin", &[0xff, 0xfe]);
        assert_eq!(
            resolve_str("asset:__test_utf8/good.txt").as_deref(),
            Some("text")
        );
        assert!(resolve_str("asset:__test_utf8/bad.bin").is_none());
    }

    #[cfg(feature = "host")]
    #[test]
    fn registered_uris_lists_compiled_in_assets_only() {
        register_bytes("__test_listed", "b.bin", b"b");
        register_bytes("__test_listed", "a.bin", b"a");
        register_resolver("__test_listed_resolver", Box::new(|_| Some(vec![1])));

        let uris = registered_uris();
        let mine: Vec<&String> = uris
            .iter()
            .filter(|u| u.starts_with("asset:__test_listed/"))
            .collect();
        assert_eq!(
            mine,
            vec!["asset:__test_listed/a.bin", "asset:__test_listed/b.bin"],
            "sorted, and both present"
        );
        assert!(
            !uris
                .iter()
                .any(|u| u.starts_with("asset:__test_listed_resolver/")),
            "a resolver is a closure, not a listing"
        );
    }
}
