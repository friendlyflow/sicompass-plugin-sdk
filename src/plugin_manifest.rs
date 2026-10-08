//! `plugin.json`: the manifest every sicompass plugin ships.
//!
//! Portable (no `host` feature), because three parties read it: the app's plugin
//! host, the `sicompass-plugin` tool that packs and signs a release, and the
//! Store that shows a plugin before installing it. The rules for what a manifest
//! may ask for, and how a built component is checked against it, are in
//! [`crate::plugin_abi`]. The design is sicompass's docs/plugin-platform.md.

use serde::{Deserialize, Serialize};

/// How the plugin is executed. `plugin.json` must say: a manifest without a
/// `type` is a WASM plugin from before sicompass 0.3, when that was the
/// default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PluginType {
    /// A program of its own, which the app starts and talks to over its stdin
    /// and stdout ([`crate::plugin_ipc`]). `entry` names the executable without
    /// an extension: the app adds `.exe` on Windows, so one `plugin.json` serves
    /// every platform. It runs with the user's rights, so its `permissions` say
    /// what it intends to do rather than limit it.
    Process,
    /// Instantiate a built-in factory provider by the manifest's `name` field.
    ///
    /// Not third-party code: this names a provider already compiled into the
    /// binary, which is why it is not sandboxed.
    Factory,
}

/// Plugin types that used to exist, kept only to explain their absence.
///
/// `native` loaded a `.so`/`.dll`/`.dylib` into the app through `dlopen`, and
/// `script` ran a `bun` script per operation. `wasm` was a sandboxed
/// WebAssembly component, the only third-party plugin until sicompass 0.3. A
/// plugin is a program now (`process`), so each of these needs a new release.
pub const RETIRED_TYPES: &[&str] = &["native", "script", "wasm"];

/// Kind of a per-plugin setting entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SettingKind {
    Text,
    /// A text setting shown masked, for keys and tokens.
    Password,
    Checkbox,
    Radio,
}

/// A single setting declared by a plugin manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginSetting {
    #[serde(rename = "type")]
    pub kind: SettingKind,
    pub label: String,
    pub key: String,
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub default_checked: bool,
    #[serde(default)]
    pub options: Vec<String>,
}

/// Parsed contents of a `plugin.json` manifest file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub name: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub plugin_type: PluginType,
    /// Relative entry path (resolved relative to the manifest directory).
    pub entry: String,
    #[serde(default)]
    pub supports_config_files: bool,
    #[serde(default)]
    pub settings: Vec<PluginSetting>,
    /// Optional plugin version, displayed in the settings tree under the
    /// plugin's section. Authors set this in `plugin.json`.
    #[serde(default)]
    pub version: Option<String>,
    /// HTTPS URL the updater queries for a newer manifest. Absent =>
    /// plugin opts out of auto-update.
    #[serde(default)]
    pub update_url: Option<String>,
    /// Minimum sicompass app version this plugin works against. If the
    /// running app is older, the plugin is skipped at load time.
    #[serde(default)]
    pub min_app_version: Option<String>,
    /// Base64-encoded ed25519 public key. Trust root for verifying
    /// signatures on future updates. First-install is trust-on-first-use.
    #[serde(default)]
    pub pubkey: Option<String>,
    /// Whether the running provider can be torn down + re-instantiated
    /// mid-session after an update lands on disk. Defaults to `true`;
    /// plugins that spawn long-lived threads holding fn-pointers from
    /// their own library must declare `false` and require a restart.
    #[serde(default = "default_hot_reload")]
    pub hot_reload: bool,
    /// Hosts a `wasm` plugin may reach over the network.
    ///
    /// This is a capability declaration, not a hint. Absent or empty means the
    /// network interface is **not linked into the guest at all**, so the plugin has
    /// no reachable network function rather than a blocked one. A component that
    /// uses the network without declaring hosts here is refused before it is
    /// instantiated.
    ///
    /// Matching is exact and case-insensitive: subdomains must be listed
    /// individually, because `evil.example.com` is not `example.com`. Listing them
    /// here is also what shows the user, before they enable the plugin, where it
    /// intends to connect.
    ///
    /// Also accepted as `permissions.allowedHosts`, where the other permissions
    /// live. The two lists are merged; see [`PluginManifest::allowed_hosts`].
    #[serde(default, rename = "allowedHosts")]
    pub top_level_allowed_hosts: Vec<String>,
    /// What the plugin may touch beyond the inert baseline every plugin gets.
    /// See docs/plugin-platform.md §4.
    #[serde(default)]
    pub permissions: Permissions,
    /// A Fluent id in the plugin's own `locales/`, describing it in one line for
    /// the Store.
    #[serde(default)]
    pub description: Option<String>,
    /// A paid service the plugin uses, shown before it is installed.
    #[serde(default)]
    pub service: Option<Service>,
    /// It renders the web pages other programs link to (a browser): the host
    /// asks it with [`crate::plugin_abi::RENDER_URL_COMMAND`]. Grants nothing.
    #[serde(default, rename = "rendersPages")]
    pub renders_pages: bool,
}

/// `permissions` in `plugin.json`. Everything defaults to "not granted".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Permissions {
    /// Hosts the `net` interface may reach (same as the top-level key).
    pub allowed_hosts: Vec<String>,
    /// A folder of its own: `app_data_dir()/<name>`, preopened.
    pub storage: bool,
    /// Folders the user grants, preopened at the same path.
    pub filesystem: Vec<String>,
    /// Programs it may start.
    pub process: Vec<String>,
    /// `host:port` pairs it may open sockets to.
    pub sockets: Vec<String>,
}

/// `service` in `plugin.json`: a paid service on a server, which the Store shows
/// before install. The plugin itself stays free (docs/plugin-platform.md §1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Service {
    /// Store tier id, for example `friendlyflow/cloud`.
    pub tier: String,
    /// A Fluent id in the plugin's `locales/`: what the service does.
    #[serde(default)]
    pub what: Option<String>,
}

impl PluginManifest {
    /// Hosts the plugin may reach: the top-level `allowedHosts` and
    /// `permissions.allowedHosts`, merged, in declaration order, without duplicates.
    pub fn allowed_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = Vec::new();
        for h in self
            .top_level_allowed_hosts
            .iter()
            .chain(&self.permissions.allowed_hosts)
        {
            if !hosts.iter().any(|x| x.eq_ignore_ascii_case(h)) {
                hosts.push(h.clone());
            }
        }
        hosts
    }
}

fn default_hot_reload() -> bool {
    true
}

/// Parse a `plugin.json`. A retired plugin type (`native`, `script`, `wasm`,
/// or no `type` at all, which meant `wasm`) gets an error that says so,
/// instead of serde's "unknown variant".
pub fn parse_manifest(json: &str) -> Result<PluginManifest, String> {
    serde_json::from_str(json).map_err(|e| {
        let value = serde_json::from_str::<serde_json::Value>(json).ok();
        let retired = value.as_ref().and_then(|v| match v.get("type") {
            None => Some("wasm".to_owned()),
            Some(t) => t
                .as_str()
                .filter(|t| RETIRED_TYPES.contains(t))
                .map(str::to_owned),
        });
        match retired {
            Some(t) => format!(
                "plugin type `{t}` is no longer supported: a plugin is a program \
                 now (`\"type\": \"process\"`), so it needs a newer release, from the Store"
            ),
            None => e.to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_parse_and_both_allowed_hosts_lists_merge() {
        let m = parse_manifest(
            r#"{
                "name": "notes", "displayName": "notes", "type": "process", "entry": "plugin",
                "allowedHosts": ["cloud.example.org"],
                "permissions": {
                    "allowedHosts": ["Cloud.example.org", "api.example.org"],
                    "storage": true
                },
                "description": "notes-description",
                "service": { "tier": "friendlyflow/cloud", "what": "notes-service" }
            }"#,
        )
        .unwrap();
        assert_eq!(
            m.allowed_hosts(),
            vec!["cloud.example.org".to_owned(), "api.example.org".to_owned()]
        );
        assert!(m.permissions.storage);
        assert_eq!(
            m.service.map(|s| s.tier).as_deref(),
            Some("friendlyflow/cloud")
        );
    }

    #[test]
    fn nothing_is_granted_by_default() {
        let m = parse_manifest(
            r#"{ "name": "x", "displayName": "x", "type": "process", "entry": "p" }"#,
        )
        .unwrap();
        assert_eq!(m.permissions, Permissions::default());
        assert!(m.allowed_hosts().is_empty());
        assert_eq!(m.plugin_type, PluginType::Process);
        assert!(m.hot_reload);
    }

    #[test]
    fn rendering_pages_is_declared_and_off_by_default() {
        let plain = parse_manifest(
            r#"{ "name": "x", "displayName": "x", "type": "process", "entry": "p" }"#,
        )
        .unwrap();
        assert!(!plain.renders_pages);
        let browser = parse_manifest(
            r#"{ "name": "x", "displayName": "x", "type": "process", "entry": "p", "rendersPages": true }"#,
        )
        .unwrap();
        assert!(browser.renders_pages);
    }

    #[test]
    fn a_process_plugin_names_its_entry_without_an_extension() {
        let m = parse_manifest(
            r#"{ "name": "x", "displayName": "x", "type": "process", "entry": "plugin" }"#,
        )
        .unwrap();
        assert_eq!(m.plugin_type, PluginType::Process);
        assert_eq!(m.entry, "plugin");
    }

    #[test]
    fn a_wasm_manifest_with_or_without_its_type_is_explained() {
        for json in [
            r#"{ "name": "x", "displayName": "x", "type": "wasm", "entry": "plugin.wasm" }"#,
            r#"{ "name": "x", "displayName": "x", "entry": "plugin.wasm" }"#,
        ] {
            let e = parse_manifest(json).unwrap_err();
            assert!(e.contains("`wasm` is no longer supported"), "{e}");
            assert!(e.contains("from the Store"), "{e}");
        }
    }

    #[test]
    fn a_retired_type_is_explained() {
        let e = parse_manifest(
            r#"{ "name": "x", "displayName": "x", "entry": "x.so", "type": "native" }"#,
        )
        .unwrap_err();
        assert!(e.contains("no longer supported"), "{e}");
    }

    #[test]
    fn a_manifest_round_trips_through_json() {
        let json = r#"{ "name": "x", "displayName": "X", "type": "process", "entry": "plugin",
                        "version": "0.2.0", "permissions": { "storage": true } }"#;
        let m = parse_manifest(json).unwrap();
        let again = parse_manifest(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(m, again);
    }
}
