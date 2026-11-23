//! xTool M1 on old firmware, over its USB network (HTTP on port 8080): status `GET /cnc/status`, the job uploaded as
//! a zip of `gcodes.txt` (`POST /cnc/data?action=upload&zip=true&id=-1`) and started by the M1's own button, stop
//! `GET /cnc/data?action=stop`. From xtm1_toolkit (MIT, ran on its author's M1; docs/research/xtool-network.md §6);
//! untried by GBurn. Each request runs on its own thread; the UI only drains the answers.
use crate::ruida_link::Phase;
use std::collections::VecDeque;
use std::sync::mpsc;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Status,
    Tool,
    Upload,
    Stop,
}

type Answer = (Kind, Result<(u16, Vec<u8>), String>);

pub struct M1 {
    pub host: String,
    tx: mpsc::Sender<Answer>,
    rx: mpsc::Receiver<Answer>,
    /// a request on its way (one at a time) and the ones after it
    busy: bool,
    queue: VecDeque<(Kind, Vec<u8>)>,
    poll_at: f64,
    /// the M1's own word: P_IDLE, P_SLEEP, P_WORKING, P_FINISH, …
    pub status: String,
    pub phase: Phase,
    pub error: String,
    pub log: VecDeque<String>,
    pub job_seconds: f64,
    started: f64,
    now: f64,
    /// polls in a row that got no answer
    misses: u32,
    /// the first status answer came back like an M1's; `dead` = it didn't (or nothing answered): the app drops the link
    checked: bool,
    pub dead: bool,
}

/// one HTTP request, blocking (call it off the UI thread, except at connect); the status code and the body
pub fn http(host: &str, method: &str, path: &str, body: &[u8], secs: u64) -> Result<(u16, Vec<u8>), String> {
    use std::io::{Read, Write};
    use std::net::ToSocketAddrs;
    let t = std::time::Duration::from_secs(secs);
    let addr = if host.contains(':') { host.to_string() } else { format!("{host}:8080") }; // a port given = a test's stand-in
    let sa = addr.to_socket_addrs().map_err(|e| format!("{addr}: {e}"))?.next().ok_or(format!("{addr}: no address"))?;
    let mut c = std::net::TcpStream::connect_timeout(&sa, t).map_err(|e| format!("{addr}: {e}"))?;
    let _ = (c.set_read_timeout(Some(t)), c.set_write_timeout(Some(t)));
    let ctype = if method == "POST" { "Content-Type: application/x-www-form-urlencoded\r\n" } else { "" };
    let head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{ctype}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    c.write_all(head.as_bytes()).and_then(|_| c.write_all(body)).map_err(|e| e.to_string())?;
    let mut r = Vec::new();
    c.read_to_end(&mut r).map_err(|e| e.to_string())?;
    let split = r.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no HTTP answer")?;
    let head = String::from_utf8_lossy(&r[..split]).to_lowercase();
    let code = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or("no HTTP status")?;
    let mut body = r[split + 4..].to_vec();
    if head.contains("transfer-encoding: chunked") {
        body = dechunk(&body);
    }
    Ok((code, body))
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(i) = b.windows(2).position(|w| w == b"\r\n") {
        let n = usize::from_str_radix(String::from_utf8_lossy(&b[..i]).split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        if n == 0 || i + 2 + n > b.len() {
            break;
        }
        out.extend_from_slice(&b[i + 2..i + 2 + n]);
        b = &b[(i + 4 + n).min(b.len())..];
    }
    out
}

/// `{"STATUS":"P_IDLE",…}` → "P_IDLE"
fn status_word(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    Some(v.get("STATUS")?.as_str()?.to_string())
}

impl M1 {
    /// the status is asked for on the first tick, off the UI thread (a wrong address used to freeze the window for
    /// seconds); an answer that isn't an M1's, or none, sets `dead` with the reason in `error`
    pub fn connect(host: &str) -> M1 {
        let status = String::new();
        let (tx, rx) = mpsc::channel();
        let mut m = M1 {
            host: host.into(),
            tx,
            rx,
            busy: false,
            queue: VecDeque::new(),
            poll_at: 0.0,
            status,
            phase: Phase::Idle,
            error: String::new(),
            log: VecDeque::new(),
            job_seconds: 0.0,
            started: 0.0,
            now: 0.0,
            misses: 0,
            checked: false,
            dead: false,
        };
        m.note(&format!("Connecting to the xTool M1 at {host}…"));
        m
    }

    fn note(&mut self, s: &str) {
        self.log.push_back(s.to_string());
        while self.log.len() > 2000 {
            self.log.pop_front();
        }
    }

    fn idle(&self) -> bool {
        matches!(self.status.as_str(), "P_IDLE" | "P_SLEEP" | "P_FINISH")
    }

    /// upload the job; the M1 runs it when its button is pressed (the beam never starts from here alone)
    pub fn start(&mut self, gcode: &str, now: f64) {
        if !self.checked {
            self.error = "Still connecting to the M1: try again in a moment".into();
            return;
        }
        if !self.idle() {
            self.error = format!("The M1 is busy ({}): wait for it to finish or Stop", self.status);
            return;
        }
        self.error.clear();
        self.queue.push_back((Kind::Tool, Vec::new()));
        self.queue.push_back((Kind::Upload, gburn_core::gcode::m1_zip(gcode)));
        (self.phase, self.started, self.job_seconds) = (Phase::Sending, now, 0.0);
        self.note(&format!("Sending the job ({} lines)", gcode.lines().count()));
    }

    pub fn stop(&mut self) {
        self.queue.clear();
        self.queue.push_front((Kind::Stop, Vec::new()));
        if self.phase != Phase::Idle {
            self.phase = Phase::Idle;
            self.note("Stopped");
        }
    }

    pub fn state_text(&self) -> &str {
        match self.phase {
            Phase::Sending if self.queue.is_empty() && !self.busy => "Press the button on the M1 when it lights blue",
            Phase::Sending => "Sending the job…",
            Phase::Running => "Running",
            Phase::Done => "Done",
            Phase::Failed => "Failed",
            _ => match self.status.as_str() {
                "" => "Connecting…",
                "P_IDLE" | "P_FINISH" => "Idle",
                "P_SLEEP" => "Asleep",
                "P_WORKING" => "Working",
                s => s,
            },
        }
    }

    pub fn tick(&mut self, now: f64) {
        self.now = now;
        if matches!(self.phase, Phase::Sending | Phase::Running) {
            self.job_seconds = now - self.started;
        }
        while let Ok((kind, r)) = self.rx.try_recv() {
            self.busy = false;
            self.answer(kind, r);
        }
        if self.busy {
            return;
        }
        let next = self.queue.pop_front().or_else(|| (now >= self.poll_at).then(|| (Kind::Status, Vec::new())));
        let Some((kind, body)) = next else { return };
        if kind == Kind::Status {
            self.poll_at = now + 1.0;
        }
        let (method, path) = match kind {
            Kind::Status => ("GET", "/cnc/status"),
            Kind::Tool => ("POST", "/setprintToolType?type=Laser"),
            Kind::Upload => ("POST", "/cnc/data?action=upload&zip=true&id=-1"),
            Kind::Stop => ("GET", "/cnc/data?action=stop"),
        };
        if kind != Kind::Status {
            self.note(&format!("> {method} {path}"));
        }
        let (host, tx) = (self.host.clone(), self.tx.clone());
        self.busy = true;
        std::thread::spawn(move || {
            let _ = tx.send((kind, http(&host, method, path, &body, if kind == Kind::Upload { 30 } else { 5 })));
        });
    }

    fn answer(&mut self, kind: Kind, r: Result<(u16, Vec<u8>), String>) {
        match (kind, r) {
            (Kind::Status, Ok((code, body))) => {
                self.misses = 0;
                let Some(s) = status_word(&body) else {
                    if !self.checked {
                        (self.dead, self.error) = (true, format!("{} answered {code} but not like an xTool M1", self.host));
                    }
                    return;
                };
                if !self.checked {
                    self.checked = true;
                    self.note(&format!("Connected to the xTool M1 at {}: {s}", self.host));
                }
                if s != self.status {
                    self.note(&format!("< {s}"));
                }
                self.status = s;
                let waiting = self.phase == Phase::Sending && self.queue.is_empty();
                if waiting && self.status == "P_WORKING" {
                    (self.phase, self.started) = (Phase::Running, self.now);
                    self.note("The M1 started the job");
                } else if self.phase == Phase::Running && self.idle() {
                    self.phase = Phase::Done;
                    self.note(&format!("Job done in {:.0} s", self.job_seconds));
                } else if self.status.contains("ERROR") {
                    self.error = format!("The M1 reports {}", self.status);
                }
            }
            (Kind::Status, Err(e)) if !self.checked => {
                (self.dead, self.error) = (true, format!("Couldn't reach the M1 at {}: {e}", self.host));
            }
            (Kind::Status, Err(e)) => {
                self.misses += 1;
                if self.misses == 5 {
                    self.error = format!("The M1 stopped answering: {e}");
                }
            }
            (Kind::Stop, r) => self.note(&format!("< stop: {}", r.map_or_else(|e| e, |(c, _)| c.to_string()))),
            (_, Ok((200, _))) => {
                if kind == Kind::Upload {
                    self.note("Job on the M1: press its button when it lights blue");
                }
            }
            (_, r) => {
                let why = r.map_or_else(|e| e, |(c, _)| format!("HTTP {c}"));
                self.queue.clear();
                self.phase = Phase::Failed;
                self.error = format!("The M1 refused the job ({kind:?}): {why}");
                let e = self.error.clone();
                self.note(&e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a pretend M1: connect reads its status, Start sends the tool then a zip of gcodes.txt and waits for the button,
    /// its status says working then finished, Stop asks for the stop
    #[test]
    fn a_job_goes_up_as_a_zip_and_waits_for_the_button() {
        use std::io::{Read, Write};
        use std::sync::{Arc, Mutex};
        let lis = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let host = format!("127.0.0.1:{}", lis.local_addr().unwrap().port());
        let seen = Arc::new(Mutex::new(Vec::<(String, Vec<u8>)>::new()));
        let state = Arc::new(Mutex::new("P_IDLE".to_string()));
        let (seen2, state2) = (seen.clone(), state.clone());
        std::thread::spawn(move || {
            for c in lis.incoming() {
                let mut c = c.unwrap();
                let mut r = Vec::new();
                let mut buf = [0u8; 65536];
                // read the head, then as many body bytes as it says
                loop {
                    let n = c.read(&mut buf).unwrap();
                    r.extend_from_slice(&buf[..n]);
                    if let Some(i) = r.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&r[..i]).to_string();
                        let len: usize = head.lines().find_map(|l| l.strip_prefix("Content-Length: ")).and_then(|v| v.parse().ok()).unwrap_or(0);
                        if r.len() >= i + 4 + len {
                            let line = head.lines().next().unwrap().to_string();
                            seen2.lock().unwrap().push((line, r[i + 4..i + 4 + len].to_vec()));
                            break;
                        }
                    }
                }
                let body = format!("{{\"STATUS\":\"{}\"}}", state2.lock().unwrap());
                let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
            }
        });
        let mut m = M1::connect(&host);
        assert_eq!(m.state_text(), "Connecting…");
        let mut t = 0.0;
        let mut run = |m: &mut M1, n: usize| {
            for _ in 0..n {
                t += 0.1;
                m.tick(t);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        run(&mut m, 10);
        assert_eq!(m.status, "P_IDLE");
        assert!(!m.dead);
        m.start("G0 X1 Y2\nG1 X3 Y4 S500 F600\n", 0.0);
        run(&mut m, 20);
        assert_eq!(m.phase, Phase::Sending);
        assert_eq!(m.state_text(), "Press the button on the M1 when it lights blue", "{:?}", m.log);
        *state.lock().unwrap() = "P_WORKING".into();
        run(&mut m, 20);
        assert_eq!(m.phase, Phase::Running);
        *state.lock().unwrap() = "P_FINISH".into();
        run(&mut m, 20);
        assert_eq!(m.phase, Phase::Done);
        m.stop();
        run(&mut m, 5);
        let seen = seen.lock().unwrap().clone();
        let lines: Vec<&str> = seen.iter().map(|(l, _)| l.as_str()).filter(|l| !l.contains("/cnc/status")).collect();
        assert_eq!(lines, ["POST /setprintToolType?type=Laser HTTP/1.1", "POST /cnc/data?action=upload&zip=true&id=-1 HTTP/1.1", "GET /cnc/data?action=stop HTTP/1.1"]);
        let zip = &seen.iter().find(|(l, _)| l.contains("upload")).unwrap().1;
        assert_eq!(&zip[30..40], b"gcodes.txt");
        // busy: no upload
        *state.lock().unwrap() = "P_WORKING".into();
        run(&mut m, 15);
        m.start("G0 X1\n", t);
        assert!(m.error.contains("busy"));
    }

    #[test]
    fn status_and_chunked_answers_read_right() {
        assert_eq!(status_word(br#"{"STATUS":"P_IDLE","x":1}"#).as_deref(), Some("P_IDLE"));
        assert_eq!(status_word(b"<html>"), None);
        assert_eq!(dechunk(b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"), b"hello world");
    }
}
