//! Example and test fixture: background tasks.
//!
//! Commands start tasks; `fetch` lists every event the UI instance received, one
//! line each, so a host test can drive the real guest and see what arrived:
//!
//! | command | argument | task |
//! |---|---|---|
//! | `count` | N | emits `0`..`N-1` as progress, returns `counted N` |
//! | `sleep` | seconds | sleeps that long (past the 10 s call limit), returns `slept` |
//! | `spin` | - | emits `running`, loops until asked to stop, returns `stopped` |
//! | `busy` | - | emits `running`, computes forever without checking; only the host can stop it |
//! | `nested` | - | tries to spawn a task from inside a task (must be refused) |
//! | `serve` | - | lives on, answering every message sent to it with `echo <message>`; `bye` ends it with `served` |
//! | `send` | `<task id> <message>` | sends the message to that task (`tasks.send`) |
//! | `cancel` | task id | cancels it |
//!
//! Lines: `started <id>`, `<id> progress <text>`, `<id> done ok <text>`,
//! `<id> done err <reason>`.
//!
//! Build: `cargo build --release --target wasm32-wasip2`.

use sicompass_pdk::{Descriptor, FfonElement, Plugin, TaskEvent, export_plugin, tasks};

struct Task {
    path: String,
    log: Vec<String>,
}

impl Plugin for Task {
    fn new() -> Self {
        Task {
            path: "/".to_owned(),
            log: Vec::new(),
        }
    }

    fn describe(&self) -> Descriptor {
        Descriptor {
            name: "task".to_owned(),
            display_name: "task".to_owned(),
            ..Default::default()
        }
    }

    fn fetch(&mut self) -> Vec<FfonElement> {
        self.log.iter().map(FfonElement::new_str).collect()
    }

    fn current_path(&self) -> &str {
        &self.path
    }

    fn set_current_path(&mut self, p: &str) {
        self.path = p.to_owned();
    }

    fn commands(&self) -> Vec<String> {
        ["count", "sleep", "spin", "busy", "nested", "serve", "send", "cancel"]
            .map(String::from)
            .to_vec()
    }

    fn handle_command(
        &mut self,
        cmd: &str,
        arg: &str,
        _elem_type: i32,
    ) -> Result<Option<FfonElement>, String> {
        if cmd == "send" {
            let (id, message) = arg.split_once(' ').unwrap_or((arg, ""));
            let id: u64 = id.parse().map_err(|_| format!("not an id: {id}"))?;
            tasks::send(id, message.as_bytes())?;
            return Ok(None);
        }
        if cmd == "cancel" {
            let id: u64 = arg.parse().map_err(|_| format!("not an id: {arg}"))?;
            tasks::cancel(id);
            return Ok(None);
        }
        let id = tasks::spawn(cmd, arg.as_bytes())?;
        self.log.push(format!("started {id}"));
        Ok(Some(FfonElement::new_str(id.to_string())))
    }

    fn run_task(&mut self, name: &str, input: &[u8]) -> Result<Vec<u8>, String> {
        let arg = String::from_utf8_lossy(input).into_owned();
        match name {
            "count" => {
                let n: u32 = arg.parse().map_err(|_| format!("not a count: {arg}"))?;
                for i in 0..n {
                    tasks::emit(i.to_string().as_bytes());
                }
                Ok(format!("counted {n}").into_bytes())
            }
            "sleep" => {
                let secs: u64 = arg.parse().map_err(|_| format!("not seconds: {arg}"))?;
                std::thread::sleep(std::time::Duration::from_secs(secs));
                Ok(b"slept".to_vec())
            }
            "spin" => {
                tasks::emit(b"running");
                while !tasks::cancelled() {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Ok(b"stopped".to_vec())
            }
            "busy" => {
                tasks::emit(b"running");
                let mut n: u64 = 0;
                loop {
                    n = n.wrapping_add(1);
                    std::hint::black_box(n);
                }
            }
            "nested" => tasks::spawn("count", b"1").map(|id| format!("spawned {id}").into_bytes()),
            "serve" => loop {
                match tasks::receive(1000) {
                    Some(m) if m == b"bye" => return Ok(b"served".to_vec()),
                    Some(m) => tasks::emit(format!("echo {}", String::from_utf8_lossy(&m)).as_bytes()),
                    None if tasks::cancelled() => return Ok(b"stopped".to_vec()),
                    None => {}
                }
            },
            other => Err(format!("no task named {other}")),
        }
    }

    fn on_task_event(&mut self, id: u64, event: TaskEvent) {
        let line = match event {
            TaskEvent::Progress(b) => format!("{id} progress {}", String::from_utf8_lossy(&b)),
            TaskEvent::Done(Ok(b)) => format!("{id} done ok {}", String::from_utf8_lossy(&b)),
            TaskEvent::Done(Err(e)) => format!("{id} done err {e}"),
        };
        self.log.push(line);
    }
}

export_plugin!(Task);
