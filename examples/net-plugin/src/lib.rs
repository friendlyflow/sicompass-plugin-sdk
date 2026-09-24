//! A plugin that uses the network, used as a test fixture for the capability model.
//!
//! Its whole purpose is to *import* `sicompass:plugin/net`. Because LTO drops
//! imports a guest never calls, a plugin only carries the network capability in its
//! binary if it genuinely uses it — which is what lets the host compare the import
//! list against `plugin.json` and refuse a mismatch before instantiating anything.
//!
//! Loading this with an empty `allowedHosts` must fail.

use sicompass_pdk::{Descriptor, FfonElement, Plugin, export_plugin, net};

struct Net;

impl Plugin for Net {
    fn new() -> Self {
        Net
    }

    fn describe(&self) -> Descriptor {
        Descriptor {
            name: "net-demo".to_owned(),
            display_name: "net demo".to_owned(),
            ..Default::default()
        }
    }

    fn fetch(&mut self) -> Vec<FfonElement> {
        let req = net::HttpRequest {
            method: "GET".to_owned(),
            url: "https://example.com/".to_owned(),
            headers: Vec::new(),
            body: None,
        };
        // The host decides whether this is even reachable, and enforces the
        // allowlist, robots.txt and quotas if it is.
        match net::fetch(&req) {
            Ok(resp) => vec![FfonElement::new_str(format!("status: {}", resp.status))],
            Err(e) => vec![FfonElement::new_str(format!("refused: {e}"))],
        }
    }
}

export_plugin!(Net);
