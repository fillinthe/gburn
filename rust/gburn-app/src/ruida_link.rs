//! A Ruida controller over the network (desktop): UDP to port 50200 from port 40200, one packet in flight at a
//! time, each answered with ACK / NAK; status and position read with DA 00 queries (replies DA 01). No threads:
//! `tick` every frame moves the state machine. Safety: a job only starts through `start`; Frame moves with the red
//! pointer, beam off; Stop sends D8 01 ahead of everything queued.
//! Over USB (`SerialStream`, the controller's FTDI serial port, per MeerK40t's usb_transport.py, MIT) the same
//! swizzled commands go out with no checksum and no ACK; DA 00 queries still get their 9-byte DA 01 reply.

use gburn_core::ruida::{self, RdOptions, ACK, ENQ, NAK};
use gburn_core::Pt;
use std::collections::VecDeque;

/// One datagram each way: a UDP socket on the desktop, or a WebSocket to GBurn Link in the browser (which turns
/// each binary message into a datagram and back).
/// The browser's Web Serial port as a Ruida byte stream (USB, like `SerialStream` on the desktop)
#[cfg(target_arch = "wasm32")]
pub struct WebSerialStream {
    told: bool,
}
#[cfg(target_arch = "wasm32")]
impl WebSerialStream {
    pub fn new() -> Self {
        WebSerialStream { told: false }
    }
}
#[cfg(target_arch = "wasm32")]
impl Datagram for WebSerialStream {
    fn send(&mut self, bytes: &[u8]) {
        let _ = gcarve_ui::webserial::write(bytes);
    }
    fn recv(&mut self) -> Option<Vec<u8>> {
        let d = gcarve_ui::webserial::take();
        (!d.is_empty()).then_some(d)
    }
    fn error(&mut self) -> Option<String> {
        match gcarve_ui::webserial::state() {
            gcarve_ui::webserial::State::Failed(e) if !self.told => {
                self.told = true;
                Some(format!("Lost the USB port: {e}"))
            }
            _ => None,
        }
    }
    fn stream(&self) -> bool {
        true
    }
}

pub trait Datagram {
    fn send(&mut self, bytes: &[u8]);
    /// the next datagram that arrived, if any
    fn recv(&mut self) -> Option<Vec<u8>>;
    /// a problem the transport itself hit (the Link not running, refused)
    fn error(&mut self) -> Option<String> {
        None
    }
    /// a byte stream (USB serial): no checksum, no ACK, replies may come in pieces
    fn stream(&self) -> bool {
        false
    }
    /// a stream still has this much unwritten: hold back
    fn busy(&self) -> bool {
        false
    }
    /// drop whatever a stream hasn't written yet (Stop goes out first)
    fn clear(&mut self) {}
    /// something seen that explains a silence (UDP: answers from another address)
    fn hint(&self) -> Option<String> {
        None
    }
}

/// The Ruida's own USB port: an FTDI serial port (0403:6001), 115200 8N1, RTS / CTS and DTR / DSR on.
#[cfg(not(target_arch = "wasm32"))]
pub struct SerialStream {
    port: Box<dyn serialport::SerialPort>,
    out: Vec<u8>,
    err: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SerialStream {
    pub fn open(path: &str) -> Result<SerialStream, String> {
        let mut port = serialport::new(path, 115_200)
            .flow_control(serialport::FlowControl::Hardware)
            .timeout(std::time::Duration::from_millis(0))
            .open()
            .map_err(|e| crate::laser::access_hint(format!("{path}: {e}")))?;
        let _ = port.write_data_terminal_ready(true);
        Ok(SerialStream { port, out: Vec::new(), err: None })
    }
    /// the first FTDI serial port (the Ruida's USB chip)
    pub fn find() -> Option<String> {
        serialport::available_ports().ok()?.into_iter().find(|p| matches!(&p.port_type, serialport::SerialPortType::UsbPort(u) if u.vid == 0x0403)).map(|p| p.port_name)
    }
    /// write what the port takes now; the rest waits (the controller holds CTS while it's busy)
    fn flush(&mut self) {
        if self.out.is_empty() {
            return;
        }
        match std::io::Write::write(&mut self.port, &self.out) {
            Ok(n) => {
                self.out.drain(..n);
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {}
            Err(e) => self.err = Some(format!("USB: {e}")),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Datagram for SerialStream {
    fn send(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
        self.flush();
    }
    fn recv(&mut self) -> Option<Vec<u8>> {
        self.flush();
        let n = self.port.bytes_to_read().unwrap_or(0) as usize;
        if n == 0 {
            return None;
        }
        let mut buf = vec![0u8; n];
        let got = std::io::Read::read(&mut self.port, &mut buf).unwrap_or(0);
        (got > 0).then(|| buf[..got].to_vec())
    }
    fn error(&mut self) -> Option<String> {
        self.err.take()
    }
    fn stream(&self) -> bool {
        true
    }
    fn busy(&self) -> bool {
        self.out.len() >= 1024
    }
    fn clear(&mut self) {
        self.out.clear();
        let _ = self.port.clear(serialport::ClearBuffer::Output);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub struct Udp {
    sock: std::net::UdpSocket,
    to: std::net::SocketAddr,
    /// a datagram came from another address (a second controller, a changed DHCP lease)
    stray: Option<std::net::IpAddr>,
    send_err: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Udp {
    pub fn open(ip: &str, to_port: u16, from_port: u16) -> Result<Udp, String> {
        let to: std::net::SocketAddr = format!("{ip}:{to_port}").parse().map_err(|_| format!("not an address: {ip}"))?;
        let sock = std::net::UdpSocket::bind(("0.0.0.0", from_port)).map_err(|e| format!("port {from_port}: {e} (is LightBurn or another sender running?)"))?;
        sock.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Udp { sock, to, stray: None, send_err: None })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Datagram for Udp {
    fn send(&mut self, bytes: &[u8]) {
        if let Err(e) = self.sock.send_to(bytes, self.to) {
            if e.kind() != std::io::ErrorKind::WouldBlock {
                self.send_err.get_or_insert_with(|| e.to_string());
            }
        }
    }
    fn recv(&mut self) -> Option<Vec<u8>> {
        let mut buf = [0u8; 1500];
        loop {
            let (n, from) = self.sock.recv_from(&mut buf).ok()?;
            if from.ip() == self.to.ip() {
                return Some(buf[..n].to_vec());
            }
            self.stray = Some(from.ip()); // not our controller's answer
        }
    }
    fn error(&mut self) -> Option<String> {
        // macOS 15+: "No route to host" when GBurn isn't allowed on the Local Network
        let mac = if cfg!(target_os = "macos") { " (Mac: System Settings › Privacy & Security › Local Network › GBurn on)" } else { "" };
        self.send_err.take().map(|e| format!("Couldn't send to {}: {e}. Check the address and that this computer is on the laser's network{mac}", self.to.ip()))
    }
    fn hint(&self) -> Option<String> {
        self.stray.map(|ip| format!(" Answers came from {ip} instead: is that the laser's address now?"))
    }
}

/// The browser: a WebSocket to GBurn Link on this computer (`ws://127.0.0.1:7451/udp?host=…`).
#[cfg(target_arch = "wasm32")]
pub struct Ws {
    sock: web_sys::WebSocket,
    inbox: std::rc::Rc<std::cell::RefCell<VecDeque<Vec<u8>>>>,
    err: std::rc::Rc<std::cell::RefCell<Option<String>>>,
    // the closures must outlive the socket
    _keep: Vec<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::MessageEvent)>>,
    _keep2: Vec<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>>,
}

#[cfg(target_arch = "wasm32")]
impl Ws {
    pub fn open(url: &str) -> Result<Ws, String> {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;
        let sock = web_sys::WebSocket::new(url).map_err(|_| format!("couldn't open {url}"))?;
        sock.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let inbox = std::rc::Rc::new(std::cell::RefCell::new(VecDeque::new()));
        let err = std::rc::Rc::new(std::cell::RefCell::new(None));
        let (ib, eb) = (inbox.clone(), err.clone());
        let on_msg = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            if let Ok(buf) = e.data().dyn_into::<js_sys::ArrayBuffer>() {
                ib.borrow_mut().push_back(js_sys::Uint8Array::new(&buf).to_vec());
            } else if let Some(t) = e.data().as_string() {
                if let Some(m) = t.strip_prefix("error: ") {
                    *eb.borrow_mut() = Some(m.to_string());
                }
            }
        });
        sock.set_onmessage(Some(on_msg.as_ref().unchecked_ref()));
        let e2 = err.clone();
        let on_err = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
            *e2.borrow_mut() = Some("GBurn Link isn't running on this computer (download it from gburn.app/download/ and start it), or it refused the address".to_string());
        });
        sock.set_onerror(Some(on_err.as_ref().unchecked_ref()));
        Ok(Ws { sock, inbox, err, _keep: vec![on_msg], _keep2: vec![on_err] })
    }
}

#[cfg(target_arch = "wasm32")]
impl Datagram for Ws {
    fn send(&mut self, bytes: &[u8]) {
        if self.sock.ready_state() == web_sys::WebSocket::OPEN {
            let _ = self.sock.send_with_u8_array(bytes);
        }
    }
    fn recv(&mut self) -> Option<Vec<u8>> {
        self.inbox.borrow_mut().pop_front()
    }
    fn error(&mut self) -> Option<String> {
        self.err.borrow_mut().take()
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for Ws {
    fn drop(&mut self) {
        let _ = self.sock.close();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// a job's packets are going out
    Sending,
    /// every packet acknowledged, the controller still reports the job running
    Running,
    Paused,
    Done,
    Failed,
}

pub struct Ruida {
    link: Box<dyn Datagram>,
    pub opts: RdOptions,
    /// packets waiting (already framed), the front one in flight
    queue: VecDeque<Vec<u8>>,
    /// how many packets at the queue's front were put there by `send_now` (pause / resume / stop): not job packets
    ctrl_front: usize,
    /// (time sent, tries left) for the packet at the queue's front
    flight: Option<(f64, u8)>,
    /// the DA 00 query whose data reply is still due, and since when
    reply_due: Option<u16>,
    reply_at: f64,
    /// USB: bytes received that don't make a whole reply yet
    rx: Vec<u8>,
    /// machine position, µm (from MEM_CURRENT_X / Y), and the status word
    pub pos: (i64, i64),
    /// where the last jog / frame point sent the head (machine µm), so quick jogs add up
    target: Option<(i64, i64)>,
    pub status: i64,
    /// the bed size the controller is set up for, µm (0 = not read yet)
    pub bed: (i64, i64),
    /// Get Controller Info: the counters read so far (address, value)
    pub info: Vec<(u16, i64)>,
    /// Controller Files: how many the controller says it holds, and the names read so far (number from 1, name)
    pub doc_count: Option<i64>,
    /// Controller Settings: values read (address, controller units)
    pub settings: Vec<(u16, i64)>,
    /// jog and frame speed, mm/s (the app sets it from Machine Settings' travel speed)
    pub move_speed: f64,
    pub files: Vec<(u16, String)>,
    /// the job going out is a file for the controller's memory, not a run
    upload: bool,
    pub seen: bool,
    /// the controller answered something since connect (`seen` resets on a timeout)
    answered: bool,
    /// NAKs in a row for the packet in flight
    naks: u8,
    pub phase: Phase,
    /// job packets total / acknowledged
    pub total: usize,
    pub acked: usize,
    /// datagrams sent since connect (resends included)
    pub sent: usize,
    last_poll: f64,
    poll_step: u8,
    pub log: VecDeque<String>,
    pub error: String,
    /// seconds the job has run
    job_started: f64,
    pub job_seconds: f64,
}

const TIMEOUT: f64 = 0.25;
const TRIES: u8 = 8;

impl Ruida {
    /// The controller's USB port (the first FTDI port when `path` is empty).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect_usb(path: &str, opts: RdOptions) -> Result<Ruida, String> {
        let path = if path.is_empty() { SerialStream::find().ok_or("No Ruida USB port found: is the cable in and the laser on?")? } else { path.to_string() };
        Self::with_link(Box::new(SerialStream::open(&path)?), &format!("USB {path}"), opts)
    }
    /// Bind the Ruida ports and remember the controller's address (nothing is sent until `tick`).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect(ip: &str, opts: RdOptions) -> Result<Ruida, String> {
        Self::connect_ports(ip, 50200, 40200, opts)
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect_ports(ip: &str, to_port: u16, from_port: u16, opts: RdOptions) -> Result<Ruida, String> {
        Self::with_link(Box::new(Udp::open(ip, to_port, from_port)?), &format!("{ip}:{to_port}"), opts)
    }
    /// The browser: through GBurn Link on this computer.
    #[cfg(target_arch = "wasm32")]
    pub fn connect(ip: &str, opts: RdOptions) -> Result<Ruida, String> {
        let url = format!("ws://127.0.0.1:7451/udp?host={ip}&port=50200&from=40200");
        Self::with_link(Box::new(Ws::open(&url)?), &format!("{ip} through GBurn Link"), opts)
    }
    pub fn with_link(link: Box<dyn Datagram>, to: &str, opts: RdOptions) -> Result<Ruida, String> {
        let mut r = Ruida {
            link,
            opts,
            queue: VecDeque::new(),
            ctrl_front: 0,
            flight: None,
            reply_due: None,
            reply_at: 0.0,
            rx: Vec::new(),
            pos: (0, 0),
            target: None,
            status: 0,
            bed: (0, 0),
            info: Vec::new(),
            doc_count: None,
            settings: Vec::new(),
            move_speed: 100.0,
            files: Vec::new(),
            upload: false,
            seen: false,
            answered: false,
            naks: 0,
            phase: Phase::Idle,
            total: 0,
            acked: 0,
            sent: 0,
            last_poll: 0.0,
            poll_step: 0,
            log: VecDeque::new(),
            error: String::new(),
            job_started: 0.0,
            job_seconds: 0.0,
        };
        r.note(&format!("Ruida at {to}: asking who is there"));
        r.query(ruida::MEM_CARD_ID);
        // the bed the controller is set up for (MeerK40t's axis range 1 / 2)
        r.query(ruida::MEM_BED_X);
        r.query(ruida::MEM_BED_Y);
        // its speeds and accelerations, for the time estimate (LightBurn waits for "Read from controller")
        for m in ruida::MOTION_SETTINGS {
            r.query(m);
        }
        Ok(r)
    }

    fn note(&mut self, s: &str) {
        self.log.push_back(s.to_string());
        while self.log.len() > 200 {
            self.log.pop_front();
        }
    }
    fn send_now(&mut self, commands: &[u8]) {
        self.queue.push_front(ruida::packet(commands, self.opts.magic));
        self.ctrl_front += 1;
        self.flight = None; // the front changed: send it next tick (a packet in flight is resent after its timeout)
    }
    fn enqueue(&mut self, commands: &[u8]) {
        self.queue.push_back(ruida::packet(commands, self.opts.magic));
    }
    fn query(&mut self, mem: u16) {
        self.enqueue(&ruida::get_setting(mem));
    }

    /// ask for the usage counters (`info` fills as the replies come)
    pub fn ask_info(&mut self) {
        self.info.clear();
        for (mem, _) in ruida::MEM_INFO {
            self.query(mem);
        }
    }

    /// read every setting in `ruida::MACHINE_SETTINGS` (`settings` fills as the replies come)
    pub fn read_settings(&mut self) {
        self.settings.clear();
        for (mem, _, _) in ruida::MACHINE_SETTINGS {
            self.query(mem);
        }
    }
    /// write one setting, then read it back
    pub fn write_setting(&mut self, mem: u16, v: i64) {
        self.enqueue(&ruida::cmd_set_setting(mem, v));
        self.settings.retain(|(a, _)| *a != mem);
        self.query(mem);
        self.note(&format!("Setting {mem:04X} = {v}"));
    }

    /// ask for the controller's file list (`files` fills as the names come)
    pub fn list_files(&mut self) {
        self.files.clear();
        self.doc_count = None;
        self.query(ruida::MEM_DOC_COUNT);
    }
    /// keep a job on the controller as `name` (run later from its panel or `run_file`)
    pub fn send_to_memory(&mut self, name: &str, commands: &[u8], now: f64) {
        self.start(&ruida::cmd_upload(name, commands), now);
        self.upload = true;
        self.note(&format!("Saving to the controller as {}", ruida::doc_name(name)));
    }
    /// run file `n` from the controller's memory
    pub fn run_file(&mut self, n: u16, now: f64) {
        self.enqueue(&ruida::cmd_run_doc(n));
        (self.total, self.acked, self.phase, self.job_started, self.job_seconds) = (0, 0, Phase::Running, now, 0.0);
        self.error.clear();
        self.note(&format!("Running file {n} from the controller"));
    }
    pub fn delete_file(&mut self, n: u16) {
        self.enqueue(&ruida::cmd_delete_doc(n));
        self.note(&format!("Deleted file {n}"));
        self.list_files();
    }

    pub fn home(&mut self) {
        self.target = None;
        self.send_now(&ruida::cmd_home_xy());
        self.note("Home");
    }
    pub fn stop(&mut self) {
        // drop the job, stop the machine
        self.queue.clear();
        self.ctrl_front = 0;
        self.flight = None;
        self.reply_due = None;
        self.link.clear();
        self.target = None;
        for _ in 0..3 {
            self.send_now(&ruida::cmd_stop()); // three times: a lost datagram mustn't lose the Stop (repeats are harmless)
        }
        if self.phase != Phase::Idle {
            self.note("Stopped");
        }
        self.phase = Phase::Idle;
    }
    pub fn pause(&mut self) {
        if matches!(self.phase, Phase::Sending | Phase::Running) {
            self.send_now(&ruida::cmd_pause());
            self.phase = Phase::Paused;
            self.note("Paused");
        }
    }
    pub fn resume(&mut self) {
        if self.phase == Phase::Paused {
            self.send_now(&ruida::cmd_resume());
            self.phase = if self.acked < self.total { Phase::Sending } else { Phase::Running };
            self.note("Resumed");
        }
    }
    /// move the head by (dx, dy) design mm from where it is (beam off)
    ///
    /// Quick clicks add up: each jog starts from where the last one was going (`target`), not from the reported position,
    /// which lags a poll (0.3 s) behind and made a second click repeat the first (the owner, 2026-10-24).
    pub fn jog(&mut self, dx: f64, dy: f64) {
        let (px, py) = self.target.unwrap_or(self.pos);
        let here = self.opts.to_design(px, py);
        let (x, y) = self.on_bed(self.opts.to_machine(gburn_core::pt(here.x + dx, here.y + dy)));
        self.target = Some((x, y));
        self.enqueue(&ruida::cmd_move_speed(self.move_speed));
        self.enqueue(&ruida::cmd_rapid_xy(x, y));
    }
    /// trace a path (box or hull) with the red pointer on, beam off
    pub fn frame_path(&mut self, pts: &[Pt]) {
        self.enqueue(&ruida::cmd_move_speed(self.move_speed));
        for p in pts {
            let (x, y) = self.on_bed(self.opts.to_machine(*p));
            self.target = Some((x, y));
            self.enqueue(&ruida::cmd_rapid_xy(x, y));
        }
        self.note("Frame (beam off)");
    }
    /// machine µm kept on the bed
    fn on_bed(&self, (x, y): (i64, i64)) -> (i64, i64) {
        (x.clamp(0, (self.opts.bed[0] * 1000.0) as i64), y.clamp(0, (self.opts.bed[1] * 1000.0) as i64))
    }
    /// send a job (unswizzled command stream from `ruida::rd_commands`)
    pub fn start(&mut self, commands: &[u8], now: f64) {
        self.target = None;
        self.queue.clear();
        self.ctrl_front = 0;
        self.flight = None;
        self.reply_due = None;
        let packets = ruida::udp_packets(commands, self.opts.magic);
        self.total = packets.len();
        self.acked = 0;
        self.queue.extend(packets);
        self.phase = Phase::Sending;
        self.upload = false;
        self.error.clear();
        self.job_started = now;
        self.job_seconds = 0.0;
        self.note(&format!("Job: {} packets, {} bytes", self.total, commands.len()));
    }
    pub fn pos_mm(&self) -> Pt {
        self.opts.to_design(self.pos.0, self.pos.1)
    }
    pub fn state_text(&self) -> &'static str {
        if !self.seen {
            "Waiting…"
        } else if self.status & ruida::STATUS_JOB_RUNNING != 0 {
            "Run"
        } else if self.status & ruida::STATUS_MOVING != 0 {
            "Jog"
        } else {
            "Idle"
        }
    }

    /// Every frame: read answers, move the queue, poll status when idle.
    pub fn tick(&mut self, now: f64) {
        if let Some(e) = self.link.error() {
            self.error = e.clone();
            self.note(&e);
            self.queue.clear();
            self.ctrl_front = 0;
            self.flight = None;
            if self.phase == Phase::Sending {
                self.phase = Phase::Failed;
            }
        }
        while let Some(d) = self.link.recv() {
            if self.link.stream() {
                self.rx.extend(d);
                continue;
            }
            let data = ruida::unswizzle(&d, self.opts.magic);
            self.seen = true;
            self.answered = true;
            if data.len() == 1 {
                match data[0] {
                    ACK if self.flight.is_none() => {} // a late answer to something no longer in flight (a query before Start)
                    ACK => self.front_done(now),
                    NAK if self.flight.is_some() => {
                        self.flight = None; // resend the front
                        self.naks += 1;
                        if self.naks >= 8 {
                            self.fail("The controller keeps refusing the data (checksum): a bad cable or Wi-Fi bridge, or a controller that scrambles differently (Machine Settings › Ruida)");
                        } else {
                            self.note("Checksum refused: resending");
                        }
                    }
                    ENQ => {}
                    other => self.note(&format!("Reply {other:02X}")),
                }
            } else if let Some((mem, v)) = ruida::parse_reply(&data) {
                self.on_reply(mem, v, now);
            } else if let Some((n, name, _)) = ruida::parse_doc_name(&data) {
                self.on_doc_name(n, name);
            }
        }
        // USB: whole 9-byte replies (and file names, up to their 0) out of the stream; a byte that starts neither is dropped
        while self.rx.len() >= 5 {
            let data = ruida::unswizzle(&self.rx[..self.rx.len().min(64)], self.opts.magic);
            if data[0] == 0xE8 {
                match ruida::parse_doc_name(&data) {
                    Some((n, name, used)) => {
                        self.rx.drain(..used);
                        self.on_doc_name(n, name);
                        continue;
                    }
                    None if data.len() < 64 => break, // the rest of the name is still coming
                    None => {
                        self.rx.remove(0);
                        continue;
                    }
                }
            }
            if self.rx.len() < 9 {
                break;
            }
            let data = &data[..9];
            if let Some((mem, v)) = ruida::parse_reply(data) {
                self.rx.drain(..9);
                self.seen = true;
                self.on_reply(mem, v, now);
            } else {
                self.rx.remove(0);
            }
        }
        if matches!(self.phase, Phase::Sending | Phase::Running | Phase::Paused) {
            self.job_seconds = now - self.job_started;
        }
        if self.reply_due.is_some() && now - self.reply_at > 2.0 {
            self.reply_due = None; // the data never came: carry on
        }
        if self.link.stream() {
            // no ACKs: write while the port takes it, stopping at a query until its reply
            while self.reply_due.is_none() && !self.link.busy() {
                let Some(p) = self.queue.front() else { break };
                let p = p[2..].to_vec();
                self.link.send(&p);
                self.sent += 1;
                self.front_done(now);
            }
        } else if self.reply_due.is_none() {
            self.send_front(now);
        }
        // status and position while nothing else is queued (not during a job upload: the controller is busy)
        if self.queue.is_empty() && now - self.last_poll > 0.3 {
            self.last_poll = now;
            let mem = [ruida::MEM_MACHINE_STATUS, ruida::MEM_CURRENT_X, ruida::MEM_CURRENT_Y][self.poll_step as usize % 3];
            self.poll_step = self.poll_step.wrapping_add(1);
            self.query(mem);
        }
    }

    /// the front packet went through (ACKed, or written to a USB stream)
    fn front_done(&mut self, now: f64) {
        self.naks = 0;
        if let Some(p) = self.queue.pop_front() {
            self.flight = None;
            let ctrl = self.ctrl_front > 0;
            self.ctrl_front = self.ctrl_front.saturating_sub(1);
            // a job packet goes on counting while paused (the upload keeps going); Resume then finds it all sent
            if !ctrl && self.acked < self.total && self.phase == Phase::Paused {
                self.acked += 1;
            } else if !ctrl && self.phase == Phase::Sending {
                self.acked += 1;
                if self.acked >= self.total && self.upload {
                    self.phase = Phase::Done;
                    self.note("Saved to the controller");
                    self.list_files();
                } else if self.acked >= self.total {
                    self.phase = Phase::Running;
                    self.note("Job sent; the machine is running it");
                }
            }
            // a DA 00 query is answered with data after the ACK
            let u = ruida::unswizzle(&p[2..], self.opts.magic);
            if u.len() >= 4 && u[0] == 0xDA && u[1] == 0x00 {
                self.reply_due = Some(((u[2] as u16) << 8) | u[3] as u16);
                self.reply_at = now;
            }
        }
    }

    fn on_reply(&mut self, mem: u16, v: i64, now: f64) {
        self.reply_due = None;
        if ruida::MACHINE_SETTINGS.iter().any(|m| m.0 == mem) {
            self.settings.retain(|(a, _)| *a != mem);
            self.settings.push((mem, v));
        }
        match mem {
            ruida::MEM_CURRENT_X => self.pos.0 = v,
            ruida::MEM_CURRENT_Y => self.pos.1 = v,
            ruida::MEM_MACHINE_STATUS => {
                self.status = v;
                if self.phase == Phase::Running && v & ruida::STATUS_JOB_RUNNING == 0 {
                    self.phase = Phase::Done;
                    self.note(&format!("Job done in {:.0} s", now - self.job_started));
                }
            }
            ruida::MEM_CARD_ID => self.note(&format!("Controller card {v:08X}{}", if v == 0x6510_6510 { " (RDC6442S)" } else { "" })),
            ruida::MEM_BED_X => {
                self.bed.0 = v;
                self.note(&format!("Bed X {} mm", v / 1000));
            }
            ruida::MEM_BED_Y => {
                self.bed.1 = v;
                self.note(&format!("Bed Y {} mm", v / 1000));
            }
            ruida::MEM_DOC_COUNT => {
                self.doc_count = Some(v);
                for n in 1..=v.clamp(0, 100) as u16 {
                    self.enqueue(&ruida::cmd_doc_name(n));
                }
            }
            m if ruida::MEM_INFO.iter().any(|(a, _)| *a == m) => {
                self.info.retain(|(a, _)| *a != m);
                self.info.push((m, v));
            }
            _ => {}
        }
    }

    fn on_doc_name(&mut self, n: u16, name: String) {
        self.seen = true;
        self.files.retain(|(k, _)| *k != n);
        self.files.push((n, name));
        self.files.sort_by_key(|f| f.0);
    }

    /// the link gave up: the queue goes, a job being sent fails, and the reason is kept for the status bar
    fn fail(&mut self, why: &str) {
        self.error = why.to_string();
        self.note(why);
        self.queue.clear();
        self.ctrl_front = 0;
        self.flight = None;
        if self.phase == Phase::Sending {
            self.phase = Phase::Failed;
        }
    }

    /// UDP: the packet in flight: send, or resend after a timeout
    fn send_front(&mut self, now: f64) {
        match self.flight {
            None => {
                if let Some(p) = self.queue.front() {
                    self.link.send(p);
                    self.sent += 1;
                    self.flight = Some((now, TRIES));
                }
            }
            Some((t, tries)) if now - t > TIMEOUT => {
                if tries == 0 {
                    // which silence: never answered (address, network, port), or stopped answering (busy, an error on its screen)
                    let why = if self.answered {
                        "The controller stopped answering: look at its panel for an error, or a job / menu holding it; if it shows nothing, its memory may be full (Laser › Controller › Controller Files)".to_string()
                    } else {
                        format!("No answer from the controller: check its address (panel: Menu › IP), the cable, that this computer is on the same network, and that nothing else (LightBurn?) holds port 40200.{}", self.link.hint().unwrap_or_default())
                    };
                    self.fail(&why);
                    self.seen = false;
                } else if let Some(p) = self.queue.front() {
                    self.link.send(p);
                    self.sent += 1;
                    self.note("No answer yet: resending");
                    self.flight = Some((now, tries - 1));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a pretend USB controller: takes raw swizzled bytes (no checksum), never ACKs, answers queries in two pieces
    /// (bytes the controller got, replies waiting, writes)
    type UsbState = std::rc::Rc<std::cell::RefCell<(Vec<u8>, VecDeque<Vec<u8>>, usize)>>;
    struct MockUsb(UsbState);
    impl Datagram for MockUsb {
        fn send(&mut self, bytes: &[u8]) {
            let mut st = self.0.borrow_mut();
            st.2 += 1;
            let u = ruida::unswizzle(bytes, 0x88);
            if u.len() >= 4 && u[0] == 0xDA && u[1] == 0x00 {
                let v = if ((u[2] as u16) << 8 | u[3] as u16) == ruida::MEM_CURRENT_X { 77_000 } else { 0 };
                let mut rep = vec![0xDA, 0x01, u[2], u[3]];
                rep.extend(ruida::encode35(v));
                let rep = ruida::swizzle(&rep, 0x88);
                st.1.push_back(rep[..4].to_vec());
                st.1.push_back(rep[4..].to_vec());
            } else {
                st.0.extend(u);
            }
        }
        fn recv(&mut self) -> Option<Vec<u8>> {
            self.0.borrow_mut().1.pop_front()
        }
        fn stream(&self) -> bool {
            true
        }
    }

    /// a pretend UDP controller: answers each datagram sent with the next scripted byte (then nothing)
    struct Scripted(VecDeque<u8>, Option<String>, usize);
    impl Datagram for Scripted {
        fn send(&mut self, _: &[u8]) {
            self.2 += 1;
        }
        fn recv(&mut self) -> Option<Vec<u8>> {
            if self.2 == 0 {
                return None;
            }
            self.2 -= 1;
            self.0.pop_front().map(|b| ruida::swizzle(&[b], RdOptions::default().magic))
        }
        fn hint(&self) -> Option<String> {
            self.1.clone()
        }
    }

    #[test]
    fn silences_and_refusals_are_told_apart() {
        let run = |replies: Vec<u8>, hint: Option<&str>| {
            let mut r = Ruida::with_link(Box::new(Scripted(replies.into(), hint.map(String::from), 0)), "test", RdOptions::default()).unwrap();
            let mut t = 0.0;
            for _ in 0..200 {
                t += 0.05;
                r.tick(t);
            }
            r.error
        };
        let e = run(vec![], Some(" Answers came from 10.0.0.9 instead"));
        assert!(e.starts_with("No answer") && e.contains("10.0.0.9"), "{e}");
        let e = run(vec![ACK], None);
        assert!(e.starts_with("The controller stopped answering"), "{e}");
        let e = run(vec![NAK; 40], None);
        assert!(e.contains("keeps refusing"), "{e}");
    }

    #[test]
    fn a_job_streams_over_usb_without_checksums_or_acks() {
        let st = std::rc::Rc::new(std::cell::RefCell::new((Vec::new(), VecDeque::new(), 0)));
        let mut r = Ruida::with_link(Box::new(MockUsb(st.clone())), "USB", RdOptions::default()).unwrap();
        let d = gburn_core::sample();
        let p = gburn_core::plan::plan(&d, gburn_core::plan::Machine::default());
        let cmds = ruida::rd_commands(&d, &p, &r.opts);
        let mut t = 0.0;
        for _ in 0..40 {
            t += 0.02;
            r.tick(t);
        }
        assert!(r.seen && r.pos.0 == 77_000, "{:?} {:?}", r.pos, r.log);
        st.borrow_mut().0.clear();
        r.start(&cmds, t);
        r.tick(t + 0.02);
        assert_eq!(r.phase, Phase::Running);
        assert_eq!(st.borrow().0, cmds, "the whole job, in order, with no checksum bytes");
    }

    /// a stand-in controller on another port: ACKs every packet, answers DA 00 queries, counts the job's bytes
    #[test]
    fn a_job_streams_to_a_pretend_controller_and_the_status_comes_back() {
        let ctl = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        ctl.set_nonblocking(true).unwrap();
        let port = ctl.local_addr().unwrap().port();
        let mut r = Ruida::connect_ports("127.0.0.1", port, 0, RdOptions::default()).unwrap();
        let d = gburn_core::sample();
        let p = gburn_core::plan::plan(&d, gburn_core::plan::Machine::default());
        let cmds = ruida::rd_commands(&d, &p, &r.opts);
        let mut got = Vec::new();
        let t = std::cell::Cell::new(0.0f64);
        let mut running = true;
        // the controller's memory: one file to start with
        let mem_files = std::cell::RefCell::new(vec!["COASTER".to_string()]);
        let written = std::cell::RefCell::new(std::collections::HashMap::<u16, i64>::new());
        let pump = |r: &mut Ruida, got: &mut Vec<u8>, running: &mut bool| {
            let mut buf = [0u8; 2000];
            while let Ok((n, from)) = ctl.recv_from(&mut buf) {
                let pk = &buf[..n];
                assert_eq!(&pk[..2], &ruida::checksum(&pk[2..]));
                let u = ruida::unswizzle(&pk[2..], 0x88);
                ctl.send_to(&ruida::swizzle(&[ACK], 0x88), from).unwrap();
                if u.len() >= 14 && u[0] == 0xDA && u[1] == 0x01 {
                    written.borrow_mut().insert(((u[2] as u16) << 8) | u[3] as u16, ruida::decode35(&u[4..9]));
                } else if u.len() >= 4 && u[0] == 0xDA && u[1] == 0x00 {
                    let mem = ((u[2] as u16) << 8) | u[3] as u16;
                    let v: i64 = match mem {
                        m if written.borrow().contains_key(&m) => written.borrow()[&m],
                        ruida::MEM_CURRENT_X => 123_000,
                        ruida::MEM_CURRENT_Y => 45_000,
                        ruida::MEM_MACHINE_STATUS => {
                            if *running {
                                1
                            } else {
                                0
                            }
                        }
                        ruida::MEM_CARD_ID => 0x6510_6510,
                        0x0203 => 42,
                        ruida::MEM_BED_X => 900_000,
                        ruida::MEM_BED_Y => 600_000,
                        ruida::MEM_DOC_COUNT => mem_files.borrow().len() as i64,
                        _ => 0,
                    };
                    let mut rep = vec![0xDA, 0x01, u[2], u[3]];
                    rep.extend(ruida::encode35(v));
                    ctl.send_to(&ruida::swizzle(&rep, 0x88), from).unwrap();
                } else if u[0] == 0xD8 && u[1] == 0x01 {
                    *running = false;
                } else if u[0] == 0xE8 && u[1] == 0x01 {
                    let n = ruida::decode14(&u[2..4]) as usize;
                    let mut rep = u[..4].to_vec();
                    rep.extend(mem_files.borrow()[n - 1].bytes());
                    rep.push(0);
                    ctl.send_to(&ruida::swizzle(&rep, 0x88), from).unwrap();
                } else if u[0] == 0xE8 && u[1] == 0x00 {
                    let n = ruida::decode14(&u[2..4]) as usize;
                    mem_files.borrow_mut().remove(n - 1);
                } else if u[0] == 0xE8 && u[1] == 0x03 {
                    *running = true;
                } else if u.starts_with(&[0xE8, 0x02, 0xE7, 0x01]) {
                    let end = u[4..].iter().position(|b| *b == 0).unwrap() + 4;
                    mem_files.borrow_mut().push(String::from_utf8_lossy(&u[4..end]).into_owned());
                    got.extend(&u[end + 1..]);
                } else {
                    got.extend(u);
                }
            }
            // loopback delivery is not instant: a real frame is 16 ms apart, give the kernel a moment here too
            std::thread::sleep(std::time::Duration::from_millis(1));
            t.set(t.get() + 0.02);
            r.tick(t.get());
        };
        for _ in 0..150 {
            pump(&mut r, &mut got, &mut running);
        }
        assert!(r.seen);
        assert_eq!(r.bed, (900_000, 600_000), "the bed size is read at connect");
        assert_eq!(r.pos, (123_000, 45_000), "log {:?} status {} queue {} flight {:?} reply_due {:?}", r.log, r.status, r.queue.len(), r.flight, r.reply_due);
        assert!(r.log.iter().any(|l| l.contains("RDC6442S")), "{:?}", r.log);
        // Get Controller Info: every counter asked for and read (jobs run = 42)
        r.ask_info();
        for _ in 0..60 {
            pump(&mut r, &mut got, &mut running);
        }
        assert_eq!(r.info.len(), ruida::MEM_INFO.len(), "{:?}", r.info);
        assert!(r.info.contains(&(0x0203, 42)));
        r.start(&cmds, t.get());
        for _ in 0..2000 {
            pump(&mut r, &mut got, &mut running);
            if r.phase == Phase::Running {
                break;
            }
        }
        assert_eq!(r.phase, Phase::Running, "{:?} {}/{}", r.phase, r.acked, r.total);
        assert_eq!(
            (got.len(), &got[..12], got.last()),
            (cmds.len(), &cmds[..12], cmds.last()),
            "the controller received the whole stream in order (first 12 bytes, length, last); sent {} acked {} of {} log {:?}",
            r.sent,
            r.acked,
            r.total,
            r.log
        );
        assert_eq!(got, cmds);
        running = false;
        for _ in 0..60 {
            pump(&mut r, &mut got, &mut running);
        }
        assert_eq!(r.phase, Phase::Done);
        // jog from the reported position: a rapid to pos + 10 mm in x
        r.jog(10.0, 0.0);
        got.clear();
        for _ in 0..20 {
            pump(&mut r, &mut got, &mut running);
        }
        // a speed first, then an absolute move (origin flag) to pos + 10 mm
        let d9 = got.windows(3).position(|w| w == [0xD9, 0x10, 0x00]).unwrap_or_else(|| panic!("{:?}", &got[..got.len().min(20)]));
        assert!(got.starts_with(&[0xC9, 0x02]) && d9 > 0, "speed before the move: {:?}", &got[..got.len().min(20)]);
        assert_eq!(ruida::decode35(&got[d9 + 3..d9 + 8]), 133_000);
        // Controller Files: list, keep this job as "my tag", delete the first, run what's left
        let names = |r: &Ruida| r.files.iter().map(|f| f.1.clone()).collect::<Vec<_>>();
        r.list_files();
        for _ in 0..40 {
            pump(&mut r, &mut got, &mut running);
        }
        assert_eq!((r.doc_count, names(&r)), (Some(1), vec!["COASTER".to_string()]), "{:?}", r.log);
        got.clear();
        r.send_to_memory("my tag", &cmds, t.get());
        for _ in 0..2000 {
            pump(&mut r, &mut got, &mut running);
            if r.phase == Phase::Done && r.files.len() == 2 {
                break;
            }
        }
        assert_eq!(names(&r), ["COASTER", "MYTAG"], "{:?}", r.log);
        assert!(got.starts_with(&cmds[..12]), "the job follows the name");
        r.delete_file(1);
        for _ in 0..60 {
            pump(&mut r, &mut got, &mut running);
        }
        assert_eq!(names(&r), ["MYTAG"]);
        running = false;
        r.run_file(1, t.get());
        for _ in 0..30 {
            pump(&mut r, &mut got, &mut running);
        }
        assert!(running, "E8 03 started the stored job");
        // Controller Settings: read them all (the bed range comes back), write X max speed, read it back
        r.read_settings();
        for _ in 0..80 {
            pump(&mut r, &mut got, &mut running);
        }
        assert_eq!(r.settings.len(), ruida::MACHINE_SETTINGS.len(), "{:?}", r.settings);
        assert!(r.settings.contains(&(0x0026, 900_000)));
        r.write_setting(0x0023, 300_000);
        for _ in 0..30 {
            pump(&mut r, &mut got, &mut running);
        }
        assert!(r.settings.contains(&(0x0023, 300_000)), "{:?}", r.settings);
    }
}
