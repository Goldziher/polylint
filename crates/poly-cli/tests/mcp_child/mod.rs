//! A spawned `poly mcp`, spoken to over stdio JSON-RPC.
//!
//! The parity tests drive the shipped binary rather than the server in process, so the wiring
//! they check is the release's: the workspace guard, enforced trust, the bundled server and the
//! host services `poly mcp` registers.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

/// The `poly` binary under test.
pub const POLY: &str = env!("CARGO_BIN_EXE_poly");

/// A running `poly mcp`, initialized.
pub struct McpChild {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpChild {
    /// Start `poly mcp` in `dir`, offline, with its caches under `cache`, and initialize it.
    // Each test binary compiles this module on its own, and not every one uses every helper.
    #[allow(dead_code)]
    pub fn start(dir: &Path, cache: &Path) -> Self {
        Self::start_with(dir, cache, &[("POLY_OFFLINE", "1".as_ref())])
    }

    /// Start `poly mcp` in `dir` with its caches under `cache` and `env` set, and initialize it.
    /// Nothing else is set, so a server started without `POLY_OFFLINE` may reach the network.
    // Each test binary compiles this module on its own, and not every one uses every helper.
    #[allow(dead_code)]
    pub fn start_with(dir: &Path, cache: &Path, env: &[(&str, &std::ffi::OsStr)]) -> Self {
        let mut child = Command::new(POLY)
            .arg("mcp")
            .current_dir(dir)
            .env_remove("POLY_OFFLINE")
            .env_remove("POLY_TRUST")
            .envs(env.iter().copied())
            .env("POLY_CACHE_HOME", cache)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn poly mcp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut server = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        let initialized = server.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "poly-parity-test", "version": "0.0.0" },
            }),
        );
        assert!(initialized.get("result").is_some(), "initialize: {initialized}");
        server.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        server
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").expect("write to poly mcp");
        self.stdin.flush().expect("flush");
    }

    /// Send a request and return the response with its id, skipping notifications.
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).expect("read from poly mcp");
            assert!(read > 0, "poly mcp closed its stdout while answering {method}");
            let message: Value = serde_json::from_str(&line).expect("poly mcp writes JSON lines");
            if message.get("id") == Some(&json!(id)) {
                return message;
            }
        }
    }

    /// `tools/call`, returning the JSON-RPC response.
    // Each test binary compiles this module on its own, and not every one uses every helper.
    #[allow(dead_code)]
    pub fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({ "name": tool, "arguments": arguments }))
    }

    /// `tools/list`'s result.
    // Each test binary compiles this module on its own, and not every one uses every helper.
    #[allow(dead_code)]
    pub fn tools(&mut self) -> Value {
        let response = self.request("tools/list", json!({}));
        response["result"].clone()
    }
}

impl Drop for McpChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
