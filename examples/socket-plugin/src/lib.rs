//! Example and test fixture: TCP through `permissions.sockets`.
//!
//! | command | argument | does |
//! |---|---|---|
//! | `echo` | `host:port` | resolves through the host, connects, sends `ping\n`, answers the reply line |
//! | `raw` | `ip:port` | connects straight to an address, skipping resolution |
//!
//! Anything refused answers `err: <reason>`.
//!
//! Build: `cargo build --release --target wasm32-wasip2`.

use std::io::{BufRead, BufReader, Write};

use sicompass_pdk::{Descriptor, FfonElement, Plugin, export_plugin, sockets};

struct Sock {
    path: String,
}

/// Send `ping` and read one line back. Through `&TcpStream` for both
/// directions: `try_clone` is not supported on `wasm32-wasip2`.
fn exchange(stream: std::net::TcpStream) -> std::io::Result<String> {
    (&stream).write_all(b"ping\n")?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    Ok(line.trim_end().to_owned())
}

impl Plugin for Sock {
    fn new() -> Self {
        Sock {
            path: "/".to_owned(),
        }
    }

    fn describe(&self) -> Descriptor {
        Descriptor {
            name: "socket".to_owned(),
            display_name: "socket".to_owned(),
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
        ["echo", "raw"].map(String::from).to_vec()
    }

    fn handle_command(
        &mut self,
        cmd: &str,
        arg: &str,
        _elem_type: i32,
    ) -> Result<Option<FfonElement>, String> {
        let (host, port) = arg.rsplit_once(':').ok_or("expected host:port")?;
        let port: u16 = port.parse().map_err(|_| "bad port".to_owned())?;
        let result = match cmd {
            "echo" => sockets::connect(host, port).and_then(exchange),
            "raw" => host
                .parse::<std::net::IpAddr>()
                .map_err(std::io::Error::other)
                .and_then(|ip| std::net::TcpStream::connect((ip, port)))
                .and_then(exchange),
            other => return Err(format!("unknown command {other}")),
        };
        Ok(Some(FfonElement::new_str(match result {
            Ok(s) => s,
            Err(e) => format!("err: {e}"),
        })))
    }
}

export_plugin!(Sock);
