//! A plugin process for `tests/plugin_process.rs`: it writes to stdout and
//! starts a child that reads stdin, which must reach neither the channel nor
//! the app's messages.

use sicompass_sdk::FfonElement;
use sicompass_sdk::plugin::{Descriptor, Plugin};

struct Noisy {
    child_read: String,
}

impl Plugin for Noisy {
    fn new() -> Self {
        println!("stdout during new");
        Noisy {
            child_read: String::new(),
        }
    }
    fn describe(&self) -> Descriptor {
        Descriptor {
            name: "noisy".into(),
            ..Default::default()
        }
    }
    fn fetch(&mut self) -> Vec<FfonElement> {
        println!("stdout during fetch");
        vec![FfonElement::new_str(format!(
            "child read [{}]",
            self.child_read
        ))]
    }
    fn execute_command(&mut self, cmd: &str, _selection: &str) -> bool {
        if cmd != "read-stdin" {
            return false;
        }
        // A child with inherited stdio: it must find stdin empty, and its
        // stdout must not land in the channel.
        let out = sicompass_sdk::plugin::command("sh")
            .args(["-c", "echo child stdout; cat"])
            .stdout(std::process::Stdio::inherit())
            .output();
        let status = sicompass_sdk::plugin::command("sh")
            .args(["-c", "head -c 64"])
            .stdout(std::process::Stdio::piped())
            .output();
        drop(out);
        self.child_read = status
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_else(|e| e.to_string());
        true
    }
}

sicompass_sdk::plugin::main!(Noisy);
