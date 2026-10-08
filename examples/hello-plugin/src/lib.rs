//! The smallest useful sicompass plugin: a greeting, a setting, a folder you
//! can enter, and a counter that runs on a thread of its own.
//!
//! `src/main.rs` makes it the program; this library is what the tests use.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sicompass_sdk::plugin::{Descriptor, FfonElement, FfonObject, Plugin, PollResult, host};

pub struct Hello {
    path: String,
    greetee: String,
    /// Seconds since the plugin started, counted on a thread.
    ticks: Arc<AtomicU64>,
    shown: u64,
}

impl Plugin for Hello {
    fn new() -> Self {
        let ticks = Arc::new(AtomicU64::new(0));
        let counter = ticks.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                counter.fetch_add(1, Ordering::Relaxed);
            }
        });
        Hello {
            path: "/".into(),
            greetee: "world".into(),
            ticks,
            shown: 0,
        }
    }

    fn init(&mut self) {
        if let Some(who) = host::get_setting("greetee") {
            self.greetee = who;
        }
    }

    fn describe(&self) -> Descriptor {
        Descriptor {
            name: "hello".into(),
            display_name: host::translate("hello-display-name"),
            ..Default::default()
        }
    }

    fn fetch(&mut self) -> Vec<FfonElement> {
        self.shown = self.ticks.load(Ordering::Relaxed);
        if self.path == "/" {
            let greeting = host::translate_args(
                "hello-greeting",
                &[("who".to_owned(), self.greetee.clone())],
            );
            let mut folder = FfonObject::new("a folder");
            folder.push(FfonElement::new_str("inside the folder"));
            vec![
                FfonElement::new_str(greeting),
                FfonElement::new_str(format!("running for {} s", self.shown)),
                FfonElement::Obj(folder),
            ]
        } else {
            vec![FfonElement::new_str("inside the folder")]
        }
    }

    /// Ask for a redraw when the counter moved, so the root stays current.
    fn poll(&mut self) -> PollResult {
        PollResult {
            at_root: self.path == "/",
            needs_refresh: self.ticks.load(Ordering::Relaxed) != self.shown,
            ..Default::default()
        }
    }

    fn current_path(&self) -> &str {
        &self.path
    }

    fn set_current_path(&mut self, p: &str) {
        self.path = p.to_owned();
    }

    fn on_setting_change(&mut self, key: &str, value: &str) {
        if key == "greetee" {
            self.greetee = value.to_owned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_greets_and_the_folder_opens() {
        let mut h = Hello::new();
        h.on_setting_change("greetee", "you");
        // Outside sicompass a translation is its own key.
        assert_eq!(h.fetch()[0], FfonElement::new_str("hello-greeting"));
        h.push_path("a folder");
        assert!(!h.poll().at_root);
        assert_eq!(h.fetch(), vec![FfonElement::new_str("inside the folder")]);
    }
}
