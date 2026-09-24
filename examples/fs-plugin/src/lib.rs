//! Example and test fixture: files through plain `std::fs`, and the `desktop`
//! interface, inside the directories a plugin is granted.
//!
//! Every operation is a command whose argument is the element key, and whose
//! answer comes back as a single string element, so a host test can drive the
//! real guest and read what happened:
//!
//! | command | argument | answer |
//! |---|---|---|
//! | `write` | path | `ok` or `err: ...` (writes `hello from fs-plugin`) |
//! | `read` | path | the file's text or `err: ...` |
//! | `list` | directory | the entry names, sorted, comma-separated |
//! | `trash`, `restore`, `open-path`, `open-url` | path or URL | `ok` or `err: ...` |
//!
//! Build: `cargo build --release --target wasm32-wasip2`.

use sicompass_pdk::{Descriptor, FfonElement, Plugin, desktop, export_plugin};

struct Fs {
    path: String,
}

fn answer<E: std::fmt::Display>(r: Result<String, E>) -> String {
    match r {
        Ok(s) => s,
        Err(e) => format!("err: {e}"),
    }
}

impl Plugin for Fs {
    fn new() -> Self {
        Fs {
            path: "/".to_owned(),
        }
    }

    fn describe(&self) -> Descriptor {
        Descriptor {
            name: "fs".to_owned(),
            display_name: "fs".to_owned(),
            ..Default::default()
        }
    }

    fn fetch(&mut self) -> Vec<FfonElement> {
        vec![FfonElement::new_str(format!(
            "storage: {}",
            if std::fs::metadata(sicompass_pdk::STORAGE_DIR).is_ok() {
                "present"
            } else {
                "absent"
            }
        ))]
    }

    fn current_path(&self) -> &str {
        &self.path
    }

    fn set_current_path(&mut self, p: &str) {
        self.path = p.to_owned();
    }

    fn commands(&self) -> Vec<String> {
        [
            "write",
            "read",
            "list",
            "trash",
            "restore",
            "open-path",
            "open-url",
        ]
        .map(String::from)
        .to_vec()
    }

    fn handle_command(
        &mut self,
        cmd: &str,
        arg: &str,
        _elem_type: i32,
    ) -> Result<Option<FfonElement>, String> {
        let text = match cmd {
            "write" => answer(std::fs::write(arg, "hello from fs-plugin").map(|_| "ok".to_owned())),
            "read" => answer(std::fs::read_to_string(arg)),
            "list" => answer(std::fs::read_dir(arg).map(|d| {
                let mut names: Vec<String> = d
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                names.join(",")
            })),
            "trash" => answer(desktop::trash(arg).map(|_| "ok".to_owned())),
            "restore" => answer(desktop::restore(arg).map(|_| "ok".to_owned())),
            "open-path" => answer(desktop::open_path(arg).map(|_| "ok".to_owned())),
            "open-url" => answer(desktop::open_url(arg).map(|_| "ok".to_owned())),
            other => return Err(format!("unknown command {other}")),
        };
        Ok(Some(FfonElement::new_str(text)))
    }
}

export_plugin!(Fs);
