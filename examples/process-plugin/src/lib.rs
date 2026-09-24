//! Example and test fixture: starting programs through `process`.
//!
//! Each command answers with one string element, so a host test can drive the
//! real guest:
//!
//! | command | argument | does |
//! |---|---|---|
//! | `run` | `program arg...` | runs it on pipes, `exit <code>: <stdout>` |
//! | `pty` | - | runs `sh` on a PTY, types a command, `exit <code>: <output>` |
//! | `stdin` | - | feeds `cat` through its stdin, answers what came back |
//! | `cwd` | directory | `sh -c pwd` there |
//! | `env` | - | `sh -c 'echo $SICOMPASS_TEST'` with that variable set |
//!
//! Anything refused answers `err: <reason>`.
//!
//! Build: `cargo build --release --target wasm32-wasip2`.

use std::time::{Duration, Instant};

use sicompass_pdk::process::{Child, PtySize};
use sicompass_pdk::{Descriptor, FfonElement, Plugin, export_plugin};

struct Proc {
    path: String,
}

/// Read until the child exits (or 5 s pass), collecting output.
fn collect(child: &Child) -> (Option<i32>, String) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut out = Vec::new();
    loop {
        out.extend(child.read(65536));
        if let Some(code) = child.try_wait() {
            // The output has all arrived by now: read what is left.
            loop {
                let rest = child.read(65536);
                if rest.is_empty() {
                    break;
                }
                out.extend(rest);
            }
            return (Some(code), String::from_utf8_lossy(&out).into_owned());
        }
        if Instant::now() > deadline {
            child.kill();
            return (None, String::from_utf8_lossy(&out).into_owned());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn exit_line(r: (Option<i32>, String)) -> String {
    match r {
        (Some(code), out) => format!("exit {code}: {out}"),
        (None, out) => format!("timeout: {out}"),
    }
}

impl Plugin for Proc {
    fn new() -> Self {
        Proc {
            path: "/".to_owned(),
        }
    }

    fn describe(&self) -> Descriptor {
        Descriptor {
            name: "process".to_owned(),
            display_name: "process".to_owned(),
            ..Default::default()
        }
    }

    fn fetch(&mut self) -> Vec<FfonElement> {
        Vec::new()
    }

    fn current_path(&self) -> &str {
        &self.path
    }

    fn set_current_path(&mut self, p: &str) {
        self.path = p.to_owned();
    }

    fn commands(&self) -> Vec<String> {
        ["run", "pty", "stdin", "cwd", "env", "unset"]
            .map(String::from)
            .to_vec()
    }

    fn handle_command(
        &mut self,
        cmd: &str,
        arg: &str,
        _elem_type: i32,
    ) -> Result<Option<FfonElement>, String> {
        let answer = match cmd {
            "run" => {
                let mut words = arg.split_whitespace().map(str::to_owned);
                let program = words.next().unwrap_or_default();
                let args: Vec<String> = words.collect();
                Child::spawn(&program, &args, None, &[], &[], None).map(|c| exit_line(collect(&c)))
            }
            "pty" => Child::spawn("sh", &[], None, &[], &[], Some(PtySize { rows: 24, cols: 80 }))
                .and_then(|c| {
                    c.write(b"echo pty-$((6*7)); exit 3\n")?;
                    Ok(exit_line(collect(&c)))
                }),
            "stdin" => Child::spawn("cat", &[], None, &[], &[], None).and_then(|c| {
                c.write(b"piped through\n")?;
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut out = Vec::new();
                while !String::from_utf8_lossy(&out).contains('\n') && Instant::now() < deadline {
                    out.extend(c.read(4096));
                    std::thread::sleep(Duration::from_millis(10));
                }
                c.kill();
                Ok(String::from_utf8_lossy(&out).into_owned())
            }),
            "cwd" => Child::spawn("sh", &["-c".into(), "pwd".into()], Some(arg), &[], &[], None)
                .map(|c| exit_line(collect(&c))),
            "env" => Child::spawn(
                "sh",
                &["-c".into(), "echo $SICOMPASS_TEST".into()],
                None,
                &[("SICOMPASS_TEST".into(), "from-the-plugin".into())],
                &[],
                None,
            )
            .map(|c| exit_line(collect(&c))),
            // `arg` names an inherited variable to remove: the program sees it
            // unset.
            "unset" => Child::spawn(
                "sh",
                &["-c".into(), format!("echo ${{{arg}:-unset}}")],
                None,
                &[],
                std::slice::from_ref(&arg.to_owned()),
                None,
            )
            .map(|c| exit_line(collect(&c))),
            other => return Err(format!("unknown command {other}")),
        };
        Ok(Some(FfonElement::new_str(match answer {
            Ok(s) => s,
            Err(e) => format!("err: {e}"),
        })))
    }
}

export_plugin!(Proc);
