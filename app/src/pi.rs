//! One `pi --mode rpc` child: JSONL commands on stdin, records on stdout, split on LF only.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::Value;

pub struct Pi {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    child: Option<Child>,
}

pub struct Spawn<'a> {
    pub bin: &'a str,
    pub env: &'a [(String, String)],
    pub cwd: &'a Path,
    pub args: &'a [&'a str],
}

impl Pi {
    /// Starts pi; `on_record` gets every stdout record, `on_exit` the last stderr lines once stdout closes.
    pub fn spawn(
        spec: Spawn<'_>,
        on_record: impl Fn(Value) + Send + 'static,
        on_exit: impl FnOnce(String) + Send + 'static,
    ) -> std::io::Result<Pi> {
        let mut command = Command::new(spec.bin);
        command
            .args(["--mode", "rpc"])
            .args(spec.args)
            .current_dir(spec.cwd)
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("pi has no stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("pi has no stderr"))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take()));

        let tail = Arc::new(Mutex::new(String::new()));
        let stderr_tail = tail.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::new();
            while reader.read_until(b'\n', &mut buf).unwrap_or(0) > 0 {
                if let Ok(mut t) = stderr_tail.lock() {
                    t.push_str(&String::from_utf8_lossy(&buf));
                    let keep = t.len().saturating_sub(4000);
                    t.drain(..keep);
                }
                buf.clear();
            }
        });

        thread::spawn(move || {
            let mut reader = BufReader::with_capacity(1 << 16, stdout);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let trimmed = line.strip_suffix(b"\n").unwrap_or(&line);
                let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(record) = serde_json::from_slice::<Value>(trimmed) {
                    on_record(record);
                }
            }
            let mut rest = String::new();
            let _ = reader.read_to_string(&mut rest);
            let last = tail.lock().map(|t| t.clone()).unwrap_or_default();
            on_exit(last);
        });

        Ok(Pi {
            stdin,
            child: Some(child),
        })
    }

    pub fn send(&self, record: &Value) -> bool {
        let Ok(mut guard) = self.stdin.lock() else {
            return false;
        };
        let Some(stdin) = guard.as_mut() else {
            return false;
        };
        let mut bytes = record.to_string().into_bytes();
        bytes.push(b'\n');
        stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_ok()
    }
}

impl Drop for Pi {
    /// Closing stdin asks pi for an orderly shutdown; a child still running after that is killed.
    fn drop(&mut self) {
        if let Ok(mut guard) = self.stdin.lock() {
            guard.take();
        }
        let Some(mut child) = self.child.take() else {
            return;
        };
        thread::spawn(move || {
            for _ in 0..30 {
                if let Ok(Some(_)) = child.try_wait() {
                    return;
                }
                thread::sleep(std::time::Duration::from_millis(100));
            }
            let _ = child.kill();
            let _ = child.wait();
        });
    }
}
