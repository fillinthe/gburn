//! The machine side: a link (serial port, or the built-in simulated GRBL controller) and the GRBL conversation over
//! it: status polling, jogging, homing, framing, streaming a job with buffer accounting. No threads: the app calls
//! `tick` every frame and the link hands over whatever bytes arrived. Safety: nothing here fires the laser except a
//! job started with `start`; `frame` moves with the beam off.

#[cfg(not(target_arch = "wasm32"))]
use crate::galvo_link::{Galvo, Phase as GPhase};
#[cfg(not(target_arch = "wasm32"))]
use crate::k40_link::K40;
use crate::ruida_link::{Phase, Ruida};
use gburn_core::{Rect, MM_PER_IN};
use gcarve_core::grbl::{self, GrblSim, GrblStreamer, MachineStatus, SimConfig};
use std::collections::VecDeque;

pub trait Link {
    fn send(&mut self, bytes: &[u8]);
    fn recv(&mut self) -> Vec<u8>;
    /// the link broke (network GRBL: the controller or GBurn Link went away)
    fn error(&mut self) -> Option<String> {
        None
    }
    /// simulated time moves with the app's clock
    fn tick(&mut self, _dt: f64) {}
    fn name(&self) -> String;
}

/// gcarve-core's grblHAL-like controller, run in the app: test the whole flow without a laser.
pub struct Sim(pub GrblSim);

impl Link for Sim {
    fn send(&mut self, bytes: &[u8]) {
        self.0.receive(bytes);
    }
    fn recv(&mut self) -> Vec<u8> {
        self.0.take_output().into_bytes()
    }
    fn tick(&mut self, dt: f64) {
        self.0.tick(dt);
    }
    fn name(&self) -> String {
        "Simulated laser".into()
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub struct Serial {
    port: Box<dyn serialport::SerialPort>,
    path: String,
    /// the first real I/O error (a pulled cable): `error()` hands it over once and the link is dropped
    err: Option<String>,
    /// bytes the port hasn't taken yet: written on, never dropped
    out: Vec<u8>,
}

/// Write as much of `out` as `w` takes now and keep the rest. A port or socket that is only busy (a write timeout, would
/// block, 0 bytes taken: Windows' "failed to write whole buffer") is not lost; any other error is, once.
#[cfg(not(target_arch = "wasm32"))]
fn pump(w: &mut dyn std::io::Write, out: &mut Vec<u8>, err: &mut Option<String>) {
    use std::io::ErrorKind::{Interrupted, TimedOut, WouldBlock};
    while !out.is_empty() {
        match w.write(out) {
            Ok(0) => break,
            Ok(k) => drop(out.drain(..k)),
            Err(e) if matches!(e.kind(), TimedOut | WouldBlock | Interrupted) => break,
            Err(e) => {
                err.get_or_insert(e.to_string());
                out.clear();
                break;
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Serial {
    pub fn open(path: &str, baud: u32, dtr: bool) -> Result<Serial, String> {
        let port = serialport::new(path, baud).dtr_on_open(dtr).timeout(std::time::Duration::from_millis(5)).open().map_err(|e| access_hint(format!("{path}: {e}")))?;
        Ok(Serial { port, path: path.to_string(), err: None, out: Vec::new() })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Link for Serial {
    fn send(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
        pump(&mut *self.port, &mut self.out, &mut self.err);
    }
    fn recv(&mut self) -> Vec<u8> {
        pump(&mut *self.port, &mut self.out, &mut self.err); // called every frame: what didn't fit before goes now
        let n = match self.port.bytes_to_read() {
            Ok(n) => n as usize,
            Err(e) => {
                self.err.get_or_insert(e.to_string());
                0
            }
        };
        if n == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; n];
        match self.port.read(&mut buf) {
            Ok(k) => {
                buf.truncate(k);
                buf
            }
            Err(_) => Vec::new(),
        }
    }
    fn error(&mut self) -> Option<String> {
        self.err.take()
    }
    fn name(&self) -> String {
        self.path.clone()
    }
}

/// GRBL over the network (FluidNC / grblHAL Telnet, usually port 23), desktop: a plain TCP stream.
#[cfg(not(target_arch = "wasm32"))]
pub struct Tcp {
    sock: std::net::TcpStream,
    addr: String,
    err: Option<String>,
    out: Vec<u8>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Tcp {
    /// "host" or "host:port" (23 when left out)
    pub fn open(addr: &str) -> Result<Tcp, String> {
        use std::net::ToSocketAddrs;
        let full = if addr.contains(':') { addr.to_string() } else { format!("{addr}:23") };
        let sa = full.to_socket_addrs().map_err(|e| format!("{full}: {e}"))?.next().ok_or(format!("{full}: no address"))?;
        let sock = std::net::TcpStream::connect_timeout(&sa, std::time::Duration::from_secs(4)).map_err(|e| format!("{full}: {e}"))?;
        sock.set_nonblocking(true).map_err(|e| e.to_string())?;
        let _ = sock.set_nodelay(true);
        Ok(Tcp { sock, addr: full, err: None, out: Vec::new() })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Link for Tcp {
    fn send(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
        pump(&mut self.sock, &mut self.out, &mut self.err);
    }
    fn recv(&mut self) -> Vec<u8> {
        use std::io::Read;
        pump(&mut self.sock, &mut self.out, &mut self.err);
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match self.sock.read(&mut buf) {
                Ok(0) => {
                    self.err = Some("the controller closed the connection".into());
                    break;
                }
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    self.err = Some(e.to_string());
                    break;
                }
            }
        }
        out
    }
    fn error(&mut self) -> Option<String> {
        self.err.take()
    }
    fn name(&self) -> String {
        format!("GRBL at {}", self.addr)
    }
}

/// GRBL over the network from the browser: GBurn Link relays a WebSocket to the controller's TCP port.
#[cfg(target_arch = "wasm32")]
pub struct WsTcp {
    ws: crate::ruida_link::Ws,
    addr: String,
}

#[cfg(target_arch = "wasm32")]
impl Link for WsTcp {
    fn send(&mut self, bytes: &[u8]) {
        use crate::ruida_link::Datagram;
        self.ws.send(bytes);
    }
    fn recv(&mut self) -> Vec<u8> {
        use crate::ruida_link::Datagram;
        let mut out = Vec::new();
        while let Some(b) = self.ws.recv() {
            out.extend_from_slice(&b);
        }
        out
    }
    fn error(&mut self) -> Option<String> {
        use crate::ruida_link::Datagram;
        self.ws.error()
    }
    fn name(&self) -> String {
        format!("GRBL at {} (through GBurn Link)", self.addr)
    }
}

/// The browser's USB serial (Web Serial through gcarve-ui): the chooser opens on connect, bytes arrive in an inbox.
#[cfg(target_arch = "wasm32")]
pub struct WebSerial {
    pub desc: String,
}

#[cfg(target_arch = "wasm32")]
impl Link for WebSerial {
    fn send(&mut self, bytes: &[u8]) {
        let _ = gcarve_ui::webserial::write(bytes);
    }
    fn recv(&mut self) -> Vec<u8> {
        gcarve_ui::webserial::take()
    }
    fn name(&self) -> String {
        if self.desc.is_empty() {
            "USB serial (Web Serial)".into()
        } else {
            self.desc.clone()
        }
    }
}

/// Serial ports on this computer (empty on the web).
/// LightBurn's Find My Laser: each serial port opened at `baud` and listened to (most GRBL boards print their banner
/// when the port opens), then asked `$I` (build info); the ports that answer like GRBL, with the answer. No reset
/// is sent (another device on a port, say a printer mid-print, is left alone). Blocking: run it on a thread.
#[cfg(not(target_arch = "wasm32"))]
pub fn find_grbl(baud: u32) -> Vec<(String, String)> {
    use std::io::{Read, Write};
    let mut found = Vec::new();
    for name in ports() {
        let Ok(mut p) = serialport::new(&name, baud).timeout(std::time::Duration::from_millis(100)).open() else { continue };
        let mut got = String::new();
        let mut buf = [0u8; 256];
        for k in 0..30 {
            if k == 15 {
                let _ = p.write_all(b"$I\r\n");
            }
            if let Ok(n) = p.read(&mut buf) {
                got.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
            if let Some(line) = got.lines().find(|l| l.contains("Grbl") || l.contains("grblHAL") || l.starts_with("[VER:")) {
                found.push((name.clone(), line.trim().to_string()));
                break;
            }
        }
    }
    found
}

pub fn ports() -> Vec<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut v: Vec<String> =
            serialport::available_ports().map(|ps| ps.into_iter().map(|p| p.port_name).filter(|n| !n.contains("Bluetooth") && !n.contains("debug-console")).collect()).unwrap_or_default();
        v.sort();
        v.dedup();
        return v;
    }
    #[allow(unreachable_code)]
    Vec::new()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    None,
    Running,
    Paused,
    Done,
    Failed,
}

pub struct Laser {
    link: Option<Box<dyn Link>>,
    /// the browser's port chooser is open (Web Serial)
    #[cfg(target_arch = "wasm32")]
    pub web_opening: bool,
    /// seconds since the Web Serial port opened, and whether it was already opened again once (a board that restarts
    /// when its port opens is "lost" in its first seconds: the browser reopens it without the chooser, like the desktop)
    #[cfg(target_arch = "wasm32")]
    web_age: f64,
    #[cfg(target_arch = "wasm32")]
    web_retried: bool,
    /// the port being opened is for a Ruida (Web Serial, its options), not GRBL
    #[cfg(target_arch = "wasm32")]
    web_ruida: Option<gburn_core::ruida::RdOptions>,
    /// a Ruida controller over the network instead of a GRBL link (the browser goes through GBurn Link)
    pub ruida: Option<Ruida>,
    /// the serial port in use (path, baud), seconds since it opened, and a reopen waiting (seconds left): a board that
    /// restarts when its port opens (native-USB ESP32 / STM32) comes back as a new device and the old handle dies
    /// ("Access denied" on Windows), so a link lost in its first seconds, before the board said anything, opens again once
    #[cfg(not(target_arch = "wasm32"))]
    serial: Option<(String, u32)>,
    #[cfg(not(target_arch = "wasm32"))]
    link_age: f64,
    #[cfg(not(target_arch = "wasm32"))]
    reopen_in: Option<f64>,
    /// an EZCAD2 galvo board over USB (desktop)
    #[cfg(not(target_arch = "wasm32"))]
    pub galvo: Option<Galvo>,
    /// a K40 (Lihuiyu M2 Nano) board over USB (desktop)
    #[cfg(not(target_arch = "wasm32"))]
    pub k40: Option<K40>,
    /// an xTool M1 on old firmware over HTTP (desktop): the job goes up, its button starts it
    #[cfg(not(target_arch = "wasm32"))]
    pub m1: Option<crate::m1_link::M1>,
    /// the bed the galvo maps the field onto (design mm)
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))] // the galvo's field (desktop only)
    pub bed: [f64; 2],
    /// the link is the simulator: its reports are inches (gcarve-core's CNC sim), a real GRBL reports mm
    pub sim: bool,
    pub status: MachineStatus,
    rx: String,
    pub log: VecDeque<String>,
    /// a note the app should show in its status bar (a port that wouldn't open, a lost link); taken each frame
    pub alert: Option<String>,
    streamer: GrblStreamer,
    pub job: Job,
    last_poll: f64,
    /// $30 read from the controller (the S value for 100 %), 0 until known
    pub s_max: f64,
    /// $$ requested: settings lines are read for $30
    settings: Vec<(i32, f64)>,
    /// seconds the job ran so far (its start time on the app clock)
    job_started: f64,
    pub job_seconds: f64,
    /// GRBL machine whose 0,0 is another corner: Some(bed width / height) mirrors X / Y (frames, jogs, the position)
    pub mirror: [Option<f64>; 2],
    /// GRBL machine wired with X and Y the other way round (Machine Settings › Swap X / Y)
    pub swap: bool,
    /// LightBurn's synchronous transfer: one line at a time, the next after its ok (controllers that drop characters)
    pub sync: bool,
    /// lines the streamer gave out that wait to be sent one by one (sync), and how many were sent this job
    held: VecDeque<String>,
    sent: usize,
    /// Marlin: oks still owed for lines sent beside the job (pause / resume), and the last line that switched the
    /// laser on (sent again on resume, after the pause's M5)
    side_oks: usize,
    last_on: String,
    /// the controller speaks Marlin (Machine Settings › Controller): no reset / $$ / ? status; one line per ok,
    /// M114 for the position, G91 G0 jogs, G28 home, M410 stop
    pub marlin: bool,
    /// an xTool S1 over USB (Machine Settings › Controller): `?` answers `MPos:x,y,z,u`, `M108` stops, `M22` pauses,
    /// `M222 S<n>` reports the state, `M53` the lid; no reset, no `$` commands (docs/research/xtool-s1.md, UNVERIFIED)
    pub xtool: bool,
    /// the S1's job: its own line-by-line queue (gcarve's streamer squeezes out spaces and takes M6 for a tool change)
    xq: VecDeque<String>,
    x_acked: usize,
    x_total: usize,
    /// an xTool F1 on old firmware over TCP 8780 (with `xtool` set): GRBL-like lines, BeamInk's `$L` … `$P` session,
    /// several lines in flight (GRBL's 127-character count), no status or position, Stop = `G0 S0` + HTTP
    /// `/processing/stop` on 8080 (docs/research/xtool-network.md §3, UNVERIFIED by GBurn)
    pub f1: bool,
    /// the F1's infrared source (`M114 S2`) instead of the blue diode (`M114 S1`)
    pub f1_ir: bool,
    f1_host: String,
    /// the F1's lines in flight (their lengths) and the job time of its last ok
    x_fly: VecDeque<usize>,
    x_ok_at: f64,
}

/// A port or USB device the user may not open: on Linux that is the dialout group (serial) or a udev rule (galvo).
#[cfg(not(target_arch = "wasm32"))]
pub fn access_hint(e: String) -> String {
    if cfg!(target_os = "linux") && (e.contains("ermission") || e.contains("EACCES") || e.contains("Access denied")) {
        format!("{e}. Linux: add yourself to the dialout group (sudo usermod -aG dialout $USER, then log out and in) for USB serial lasers; a galvo (9588:9899) or a K40 (1a86:5512) needs a udev rule (see the manual's Connect page)")
    } else {
        e
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn SetThreadExecutionState(flags: u32) -> u32;
}

/// While a job runs the computer mustn't sleep: a sleeping PC or Mac drops the USB link mid-burn (a common forum
/// report). Windows: SetThreadExecutionState (ES_CONTINUOUS | ES_SYSTEM_REQUIRED); Mac: `caffeinate -i -w` our pid;
/// Linux: `systemd-inhibit` holding a `tail --pid` that ends with us (no systemd = nothing, quietly).
#[cfg(not(target_arch = "wasm32"))]
fn keep_awake(on: bool) {
    static AWAKE: std::sync::Mutex<Option<Option<std::process::Child>>> = std::sync::Mutex::new(None);
    let Ok(mut g) = AWAKE.lock() else { return };
    if g.is_some() == on {
        return;
    }
    if on {
        #[cfg(windows)]
        unsafe {
            SetThreadExecutionState(0x8000_0001);
        }
        #[cfg(target_os = "macos")]
        let child = std::process::Command::new("/usr/bin/caffeinate").args(["-i", "-w", &std::process::id().to_string()]).spawn().ok();
        #[cfg(target_os = "linux")]
        let child = std::process::Command::new("systemd-inhibit")
            .args(["--what=sleep:idle", "--who=GBurn", "--why=A laser job is running", "tail", &format!("--pid={}", std::process::id()), "-f", "/dev/null"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok();
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let child = None;
        *g = Some(child);
    } else {
        #[cfg(windows)]
        unsafe {
            SetThreadExecutionState(0x8000_0000);
        }
        if let Some(Some(mut c)) = g.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Default for Laser {
    fn default() -> Self {
        Laser {
            ruida: None,
            #[cfg(not(target_arch = "wasm32"))]
            serial: None,
            #[cfg(not(target_arch = "wasm32"))]
            link_age: 0.0,
            #[cfg(not(target_arch = "wasm32"))]
            reopen_in: None,
            #[cfg(target_arch = "wasm32")]
            web_opening: false,
            #[cfg(target_arch = "wasm32")]
            web_age: 0.0,
            #[cfg(target_arch = "wasm32")]
            web_retried: false,
            #[cfg(target_arch = "wasm32")]
            web_ruida: None,
            #[cfg(not(target_arch = "wasm32"))]
            galvo: None,
            #[cfg(not(target_arch = "wasm32"))]
            k40: None,
            #[cfg(not(target_arch = "wasm32"))]
            m1: None,
            bed: [600.0, 400.0],
            link: None,
            sim: false,
            status: MachineStatus::default(),
            rx: String::new(),
            log: VecDeque::new(),
            alert: None,
            streamer: GrblStreamer::default(),
            job: Job::None,
            last_poll: 0.0,
            s_max: 0.0,
            settings: Vec::new(),
            job_started: 0.0,
            job_seconds: 0.0,
            sync: false,
            mirror: [None, None],
            swap: false,
            held: VecDeque::new(),
            side_oks: 0,
            last_on: String::new(),
            sent: 0,
            marlin: false,
            xtool: false,
            xq: VecDeque::new(),
            x_acked: 0,
            x_total: 0,
            f1: false,
            f1_ir: false,
            f1_host: String::new(),
            x_fly: VecDeque::new(),
            x_ok_at: 0.0,
        }
    }
}

const LOG_LINES: usize = 3000;

fn trim_num(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// GRBL 1.1 settings a laser user meets (name, unit); others show by number only
pub fn setting_name(n: i32) -> (&'static str, &'static str) {
    match n {
        0 => ("Step pulse", "µs"),
        1 => ("Step idle delay", "ms"),
        2 => ("Step pulse invert", "mask"),
        3 => ("Direction invert", "mask"),
        4 => ("Invert step enable", "0/1"),
        5 => ("Invert limit pins", "0/1"),
        6 => ("Invert probe pin", "0/1"),
        10 => ("Status report", "mask"),
        11 => ("Junction deviation", "mm"),
        12 => ("Arc tolerance", "mm"),
        13 => ("Report in inches", "0/1"),
        20 => ("Soft limits", "0/1"),
        21 => ("Hard limits", "0/1"),
        22 => ("Homing cycle", "0/1"),
        23 => ("Homing direction invert", "mask"),
        24 => ("Homing feed", "mm/min"),
        25 => ("Homing seek", "mm/min"),
        26 => ("Homing debounce", "ms"),
        27 => ("Homing pull-off", "mm"),
        30 => ("Max laser power (S)", "S"),
        31 => ("Min laser power (S)", "S"),
        32 => ("Laser mode (keep 1)", "0/1"),
        100 => ("X steps per mm", "steps"),
        101 => ("Y steps per mm", "steps"),
        102 => ("Z steps per mm", "steps"),
        110 => ("X max rate", "mm/min"),
        111 => ("Y max rate", "mm/min"),
        112 => ("Z max rate", "mm/min"),
        120 => ("X acceleration", "mm/s²"),
        121 => ("Y acceleration", "mm/s²"),
        122 => ("Z acceleration", "mm/s²"),
        130 => ("X max travel", "mm"),
        131 => ("Y max travel", "mm"),
        132 => ("Z max travel", "mm"),
        _ => ("", ""),
    }
}

impl Laser {
    /// the browser's port chooser is up or the port is being opened (never on the desktop)
    pub fn opening(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            self.web_opening
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.reopen_in.is_some()
    }
    pub fn connected(&self) -> bool {
        self.link.is_some() || self.is_ruida() || self.is_galvo() || self.is_k40() || self.is_m1()
    }
    pub fn is_m1(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.m1.is_some()
        }
        #[cfg(target_arch = "wasm32")]
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect_m1(&mut self, host: &str) -> Result<(), String> {
        self.disconnect();
        self.m1 = Some(crate::m1_link::M1::connect(host));
        Ok(())
    }
    pub fn is_k40(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.k40.is_some()
        }
        #[cfg(target_arch = "wasm32")]
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect_k40(&mut self, usb: Box<dyn crate::k40_link::Ch341>, bed: [f64; 2], chip: u16) {
        self.disconnect();
        self.k40 = Some(K40::new(usb, bed, chip));
    }
    pub fn is_galvo(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.galvo.is_some()
        }
        #[cfg(target_arch = "wasm32")]
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect_galvo(&mut self, usb: Box<dyn crate::galvo_link::Usb>, opts: gburn_core::galvo::GalvoOptions, cor: Option<&gburn_core::galvo::CorTable>, bed: [f64; 2]) {
        self.disconnect();
        self.bed = bed;
        self.galvo = Some(Galvo::new(usb, opts, cor));
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start_galvo(&mut self, l: &gburn_core::galvo::ListBuilder, now: f64) {
        if let Some(g) = &mut self.galvo {
            g.start(l, now);
        }
    }
    pub fn is_ruida(&self) -> bool {
        self.ruida.is_some()
    }
    /// the bed size a Ruida controller reported (mm), once both axes are in
    pub fn controller_bed(&self) -> Option<[f64; 2]> {
        self.ruida.as_ref().filter(|r| r.bed.0 > 0 && r.bed.1 > 0).map(|r| [r.bed.0 as f64 / 1000.0, r.bed.1 as f64 / 1000.0])
    }
    /// a Ruida's motion settings once all are in and plausible: (cutting accel, engrave accel = X max accel, [X, Y max speed])
    /// in mm/s² and mm/s. The addresses are MeerK40t's emulator's, unverified on a real controller, so the app only offers them.
    pub fn controller_motion(&self) -> Option<(f64, f64, [f64; 2])> {
        let r = self.ruida.as_ref()?;
        let get = |m: u16| r.settings.iter().find(|(a, _)| *a == m).map(|(_, v)| *v as f64 / 1000.0);
        let [vx, vy, ax, ay, cut] = gburn_core::ruida::MOTION_SETTINGS.map(get);
        let (vx, vy, ax, cut) = (vx?, vy?, ax?, cut.filter(|c| *c > 0.0).or(ax.zip(ay).map(|(x, y)| x.min(y)))?);
        let ok = |v: f64, lo: f64, hi: f64| (lo..=hi).contains(&v);
        (ok(vx, 10.0, 5000.0) && ok(vy, 10.0, 5000.0) && ok(ax, 100.0, 100_000.0) && ok(cut, 100.0, 100_000.0)).then_some((cut, ax, [vx, vy]))
    }
    /// a GRBL link (serial, Web Serial or the simulator)
    pub fn is_grbl(&self) -> bool {
        self.link.is_some() && !self.marlin && !self.xtool
    }
    /// a G-code link of either kind (GRBL family or Marlin): frames and streams the same way
    pub fn is_gcode(&self) -> bool {
        self.link.is_some()
    }
    /// what `$$` answered, by number
    pub fn settings(&self) -> &[(i32, f64)] {
        &self.settings
    }
    pub fn read_settings(&mut self) {
        self.settings.clear();
        self.send_line("$$");
    }
    /// `$n=value`; not while a job runs (GRBL refuses it then anyway)
    pub fn write_setting(&mut self, n: i32, v: f64) {
        if self.job == Job::Running || self.job == Job::Paused {
            return self.note("Settings can't change during a job");
        }
        self.send_line(&format!("${n}={}", trim_num(v)));
        match self.settings.iter_mut().find(|(k, _)| *k == n) {
            Some(s) => s.1 = v,
            None => self.settings.push((n, v)),
        }
        if n == 30 {
            self.s_max = v;
        }
    }
    pub fn name(&self) -> String {
        #[cfg(not(target_arch = "wasm32"))]
        if self.galvo.is_some() {
            return "Galvo".into();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.k40.is_some() {
            return "K40".into();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(m) = &self.m1 {
            return format!("xTool M1 at {}", m.host);
        }
        if let Some(r) = &self.ruida {
            return format!("Ruida {}", r.opts.bed[0]);
        }
        let n = self.link.as_ref().map(|l| l.name()).unwrap_or_default();
        if self.marlin {
            n.replace("GRBL", "Marlin")
        } else {
            n
        }
    }
    /// The job state, whichever controller
    pub fn job_state(&self) -> Job {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &self.galvo {
            return match g.phase {
                GPhase::Init | GPhase::Idle => Job::None,
                GPhase::Sending | GPhase::Running => Job::Running,
                GPhase::Paused => Job::Paused,
                GPhase::Done => Job::Done,
                GPhase::Failed => Job::Failed,
            };
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(m) = &self.m1 {
            return match m.phase {
                Phase::Idle => Job::None,
                Phase::Sending | Phase::Running => Job::Running,
                Phase::Paused => Job::Paused,
                Phase::Done => Job::Done,
                Phase::Failed => Job::Failed,
            };
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &self.k40 {
            return match k.phase {
                Phase::Idle => Job::None,
                Phase::Sending | Phase::Running => Job::Running,
                Phase::Paused => Job::Paused,
                Phase::Done => Job::Done,
                Phase::Failed => Job::Failed,
            };
        }
        if let Some(r) = &self.ruida {
            return match r.phase {
                Phase::Idle => Job::None,
                Phase::Sending | Phase::Running => Job::Running,
                Phase::Paused => Job::Paused,
                Phase::Done => Job::Done,
                Phase::Failed => Job::Failed,
            };
        }
        self.job
    }
    /// "Idle" / "Run" / "Alarm"… and whether the controller has answered yet
    pub fn state_text(&self) -> (String, bool) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &self.galvo {
            return (g.state_text().to_string(), g.phase != GPhase::Init);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &self.k40 {
            return (k.state_text().to_string(), true);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(m) = &self.m1 {
            return (m.state_text().to_string(), true);
        }
        if let Some(r) = &self.ruida {
            return (r.state_text().to_string(), r.seen);
        }
        (self.status.state.clone(), self.status.valid)
    }
    pub fn log_lines(&self) -> &VecDeque<String> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &self.galvo {
            return &g.log;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &self.k40 {
            return &k.log;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(m) = &self.m1 {
            return &m.log;
        }
        if let Some(r) = &self.ruida {
            return &r.log;
        }
        &self.log
    }

    pub fn connect_ruida(&mut self, ip: &str, opts: gburn_core::ruida::RdOptions) -> Result<(), String> {
        self.disconnect();
        self.ruida = Some(Ruida::connect(ip, opts)?);
        Ok(())
    }
    /// The browser: a Ruida's USB port through Web Serial (from the Connect click; the port chooser opens)
    #[cfg(target_arch = "wasm32")]
    pub fn connect_ruida_webserial(&mut self, opts: gburn_core::ruida::RdOptions) -> Result<(), String> {
        if !gcarve_ui::webserial::supported() {
            return Err("this browser has no Web Serial: use Chrome, Edge or Brave on a computer, or the desktop app".into());
        }
        self.disconnect();
        gcarve_ui::webserial::open_hardware_flow(115_200);
        (self.web_opening, self.web_ruida) = (true, Some(opts));
        self.note("Pick the laser's USB port in the browser's list…");
        Ok(())
    }
    /// The Ruida's own USB port (the first FTDI serial port)
    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect_ruida_usb(&mut self, opts: gburn_core::ruida::RdOptions) -> Result<(), String> {
        self.disconnect();
        self.ruida = Some(Ruida::connect_usb("", opts)?);
        Ok(())
    }
    /// Start a Ruida job (the unswizzled command stream)
    pub fn start_rd(&mut self, commands: &[u8], now: f64) {
        if let Some(r) = &mut self.ruida {
            r.start(commands, now);
        }
    }

    /// The simulated laser: a bed of `bed` mm, moving at `rapid` mm/s, homed already (no alarm to clear).
    pub fn connect_sim(&mut self, bed: [f64; 2], rapid: f64) {
        self.attach(
            Box::new(Sim(GrblSim::new(SimConfig {
                travel: gcarve_core::gcode::Vec3 { x: bed[0] / MM_PER_IN, y: bed[1] / MM_PER_IN, z: 1.0 },
                rapid_ipm: rapid * 60.0 / MM_PER_IN,
                homing_required: false,
                probe_z: f64::NAN,
                probe_tilt: [0.0, 0.0],
                count_down: false,
            }))),
            true,
        );
    }

    /// GRBL over the network: "host" or "host:port" (Telnet 23); the browser goes through GBurn Link.
    pub fn connect_tcp(&mut self, addr: &str) -> Result<(), String> {
        let full = if self.f1 && !addr.contains(':') { format!("{addr}:8780") } else { addr.to_string() };
        let addr = full.as_str();
        self.f1_host = addr.split(':').next().unwrap_or("").to_string();
        #[cfg(not(target_arch = "wasm32"))]
        let link: Box<dyn Link> =
            Box::new(Tcp::open(addr).map_err(|e| if self.f1 { format!("{e} (xTool firmware 40.51 and later closed port 8780: GBurn only reaches an F1 on older firmware)") } else { e })?);
        #[cfg(target_arch = "wasm32")]
        let link: Box<dyn Link> = {
            let (host, port) = addr.split_once(':').unwrap_or((addr, "23"));
            Box::new(WsTcp { ws: crate::ruida_link::Ws::open(&format!("ws://127.0.0.1:7451/tcp?host={host}&port={port}"))?, addr: addr.to_string() })
        };
        self.attach(link, false);
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect_serial(&mut self, path: &str, baud: u32) -> Result<(), String> {
        let s = Serial::open(path, baud, !self.xtool)?; // xTool's own LightBurn profile opens with DTR off
        self.attach(Box::new(s), false);
        self.serial = Some((path.to_string(), baud));
        self.link_age = 0.0;
        Ok(())
    }

    fn attach(&mut self, link: Box<dyn Link>, sim: bool) {
        self.disconnect();
        self.link = Some(link);
        self.sim = sim;
        self.note(&format!("Connected to {}", self.name()));
        if self.marlin {
            self.send_line("M115"); // the firmware's name and features
            return;
        }
        if self.f1 {
            self.status.state = "Idle".into(); // the F1 reports nothing on 8780: ready once it took the connection
            return;
        }
        if self.xtool {
            self.send_line("?"); // the position; nothing that resets or homes
            return;
        }
        // GRBL answers a soft reset with its banner; then ask for the settings ($30 = full power S value)
        self.realtime(grbl::RESET);
        self.send_line("$$");
    }

    /// The browser: show the port chooser (must be called from the Connect click) and attach once it opens.
    #[cfg(target_arch = "wasm32")]
    pub fn connect_webserial(&mut self, baud: u32) -> Result<(), String> {
        if !gcarve_ui::webserial::supported() {
            return Err("this browser has no Web Serial: use Chrome or Edge on a computer, or the desktop app".into());
        }
        self.disconnect();
        gcarve_ui::webserial::open(baud);
        self.web_opening = true;
        self.web_retried = false;
        self.note("Pick the laser's USB port in the browser's list…");
        Ok(())
    }

    pub fn disconnect(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        keep_awake(false);
        #[cfg(target_arch = "wasm32")]
        {
            if self.web_opening || self.link.is_some() || self.ruida.is_some() {
                gcarve_ui::webserial::close();
            }
            self.web_opening = false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(mut g) = self.galvo.take() {
            g.stop();
            self.log.extend(g.log.drain(..));
            self.note("Disconnected from the galvo");
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(mut m) = self.m1.take() {
            if matches!(m.phase, Phase::Sending | Phase::Running) {
                m.stop();
                // straight out on its own thread: the link's queue waits behind a status request still on its way
                let host = m.host.clone();
                std::thread::spawn(move || crate::m1_link::http(&host, "GET", "/cnc/data?action=stop", &[], 3));
            }
            self.log.extend(m.log.drain(..));
            self.note("Disconnected from the M1");
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(mut k) = self.k40.take() {
            k.stop();
            self.log.extend(k.log.drain(..));
            self.note("Disconnected from the K40");
        }
        if let Some(mut r) = self.ruida.take() {
            r.stop();
            r.tick(f64::MAX); // the stop goes out
            self.log.extend(r.log.drain(..));
            self.note("Disconnected from the Ruida");
        }
        if self.link.is_some() {
            if self.f1 {
                self.send_line("G0 S0"); // BeamInk's safe line on close
            } else if self.xtool {
                if matches!(self.job, Job::Running | Job::Paused) {
                    self.send_line("M108");
                }
            } else if self.marlin {
                self.send_line("M5");
                self.send_line("M107");
            } else {
                self.realtime(grbl::RESET);
            }
            self.note("Disconnected");
        }
        self.link = None;
        self.status = MachineStatus::default();
        self.streamer.stop();
        self.job = Job::None;
        self.rx.clear();        #[cfg(not(target_arch = "wasm32"))]
        {
            self.serial = None;
            self.reopen_in = None;
        }
    }

    fn note(&mut self, s: &str) {
        self.log.push_back(s.to_string());
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }

    /// One line to the controller (logged as "> line").
    pub fn send_line(&mut self, line: &str) {
        if let Some(l) = &mut self.link {
            l.send(format!("{line}\n").as_bytes());
            self.note(&format!("> {line}"));
        }
    }
    /// GRBL: jog one other axis (Z focus, A rotary) by `d` (mm or degrees)
    pub fn jog_axis(&mut self, axis: char, d: f64, feed: f64) {
        if self.marlin {
            return self.send_lines(&["G91".into(), format!("G0 {axis}{d:.3} F{feed:.0}"), "G90".into()]);
        }
        if self.is_grbl() {
            self.send_line(&format!("$J=G91 G21 {axis}{d:.3} F{feed:.0}"));
        }
    }
    /// GRBL: cancel a jog in progress (hold-to-move let go)
    pub fn jog_cancel(&mut self) {
        if self.is_grbl() {
            self.realtime(0x85);
        }
    }
    fn send_lines(&mut self, lines: &[String]) {
        for l in lines {
            self.send_line(l);
        }
    }
    pub fn realtime(&mut self, b: u8) {
        if let Some(l) = &mut self.link {
            l.send(&[b]);
        }
    }

    pub fn home(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &mut self.galvo {
            let (bx, by) = (self.bed[0] / 2.0, self.bed[1] / 2.0);
            let here = g.pos_mm(self.bed);
            return g.jog(bx - here.x, by - here.y, self.bed); // the field's centre
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &mut self.k40 {
            return k.home();
        }
        if self.is_m1() {
            self.alert = Some("The M1 homes by itself at each job".into());
            return;
        }
        if let Some(r) = &mut self.ruida {
            return r.home();
        }
        if self.f1 {
            // a galvo has no home: the mirrors to the field's centre, beam off
            let c = format!("G0 X{:.3} Y{:.3}", self.bed[0] / 2.0, self.bed[1] / 2.0);
            return self.send_lines(&self.f1_session(vec![c]));
        }
        self.send_line(if self.xtool {
            "M111 S3" // X / Y (ha-xtool, MIT; UNVERIFIED over USB)
        } else if self.marlin {
            "G28 X Y"
        } else {
            "$H"
        });
    }
    pub fn unlock(&mut self) {
        self.send_line("$X");
    }
    /// relative jog, mm, at `feed` mm/min; the laser is off in jogs
    pub fn jog(&mut self, dx: f64, dy: f64, feed: f64) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &mut self.galvo {
            return g.jog(dx, dy, self.bed);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &mut self.k40 {
            return k.jog(dx, dy);
        }
        if self.is_m1() {
            self.alert = Some("Jogging isn't available on the M1 from GBurn (its single commands aren't confirmed)".into());
            return;
        }
        if let Some(r) = &mut self.ruida {
            return r.jog(dx, dy);
        }
        let (dx, dy) = (if self.mirror[0].is_some() { -dx } else { dx }, if self.mirror[1].is_some() { -dy } else { dy });
        let (dx, dy) = if self.swap { (dy, dx) } else { (dx, dy) };
        if self.f1 {
            return self.send_lines(&self.f1_session(vec!["G91".into(), format!("G0 X{dx:.3} Y{dy:.3}"), "G90".into()]));
        }
        if self.xtool {
            // a G1 at S0 with no M4 in force: beam off (LightBurn frames an S1 the same way)
            return self.send_lines(&["G91".into(), format!("G1 X{dx:.3} Y{dy:.3} S0 F{feed:.0}"), "G90".into()]);
        }
        if self.marlin {
            return self.send_lines(&["G91".into(), format!("G0 X{dx:.3} Y{dy:.3} F{feed:.0}"), "G90".into()]);
        }
        self.send_line(&format!("$J=G91 G21 X{dx:.3} Y{dy:.3} F{feed:.0}"));
    }
    /// run the outline of `r` with the beam off (M5 first, rapids only)
    /// `s`: GRBL S to frame with the beam on (0 = off)
    pub fn frame(&mut self, r: Rect, feed: f64, s: f64) {
        if r.is_empty() {
            return;
        }
        self.frame_path(&[gburn_core::pt(r.x0, r.y0), gburn_core::pt(r.x1, r.y0), gburn_core::pt(r.x1, r.y1), gburn_core::pt(r.x0, r.y1), gburn_core::pt(r.x0, r.y0)], feed, s);
    }
    /// run `pts` (a box or the design's hull) with the beam off
    pub fn frame_path(&mut self, pts: &[gburn_core::Pt], feed: f64, s: f64) {
        if pts.len() < 2 {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &mut self.galvo {
            return g.frame(pts, self.bed, self.job_started);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &mut self.k40 {
            return k.frame(pts);
        }
        if self.is_m1() {
            self.alert = Some("Frame isn't available on the M1 from GBurn: check the placement with xTool's camera view or Simulate".into());
            return;
        }
        if let Some(rd) = &mut self.ruida {
            return rd.frame_path(pts);
        }
        if self.f1 {
            // the box in M4 at the frame power (Machine Settings › % beam while framing; 0 = beam off, nothing shows)
            let (mx, my) = (self.mirror[0], self.mirror[1]);
            let body = pts
                .iter()
                .map(|p| {
                    let (a, b) = (mx.map_or(p.x, |w| w - p.x), my.map_or(p.y, |h| h - p.y));
                    let (a, b) = if self.swap { (b, a) } else { (a, b) };
                    format!("G1 X{a:.3} Y{b:.3} S{} F{feed:.0}", s.round())
                })
                .collect();
            return self.send_lines(&self.f1_session(body));
        }
        if self.xtool {
            // LightBurn's S1 frame: motors on, the red cross on, S0 moves, motors released (no M4: beam off)
            self.send_lines(&["M17 S1".into(), "M106 S1".into(), "G21 G90".into()]);
            let (mx, my) = (self.mirror[0], self.mirror[1]);
            for p in pts {
                let (a, b) = (mx.map_or(p.x, |w| w - p.x), my.map_or(p.y, |h| h - p.y));
                let (a, b) = if self.swap { (b, a) } else { (a, b) };
                self.send_line(&format!("G1 X{a:.3} Y{b:.3} S0 F6000"));
            }
            return self.send_line("M18");
        }
        self.send_line("M5");
        self.send_line("G21 G90");
        let (mx, my) = (self.mirror[0], self.mirror[1]);
        for (i, p) in pts.iter().enumerate() {
            let (a, b) = (mx.map_or(p.x, |w| w - p.x), my.map_or(p.y, |h| h - p.y));
            let (a, b) = if self.swap { (b, a) } else { (a, b) };
            self.send_line(&format!("G1 X{a:.3} Y{b:.3} F{feed:.0}"));
            if i == 0 && s > 0.0 {
                self.send_line(&format!("M3 S{s}")); // on at the first corner, constant power
            }
        }
        if s > 0.0 {
            self.send_line("M5 S0");
        }
    }

    /// Start streaming a job (the explicit Start: the only way the beam comes on).
    pub fn start(&mut self, lines: &[String], now: f64) {
        if !self.connected() {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(m) = &mut self.m1 {
            return m.start(&lines.join("\n"), now);
        }
        if self.xtool {
            self.xq = lines.iter().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && !l.starts_with(';')).collect();
            (self.x_total, self.x_acked, self.sent, self.x_ok_at) = (self.xq.len(), 0, 0, 0.0);
            self.x_fly.clear();
            self.job = Job::Running;
            self.job_started = now;
            self.job_seconds = 0.0;
            return self.note(&format!("Job started: {} lines", self.x_total));
        }
        self.streamer.load(lines);
        self.streamer.start();
        (self.held, self.sent, self.side_oks, self.last_on) = (VecDeque::new(), 0, 0, String::new());
        self.job = Job::Running;
        self.job_started = now;
        self.job_seconds = 0.0;
        self.note(&format!("Job started: {} lines", self.streamer.total()));
    }
    pub fn pause(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &mut self.galvo {
            return g.pause();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &mut self.k40 {
            return k.pause();
        }
        if self.is_m1() {
            self.alert = Some("Pause the M1 with its own button".into());
            return;
        }
        if let Some(r) = &mut self.ruida {
            return r.pause();
        }
        if self.job == Job::Running {
            if self.f1 {
                // nothing to send: the lines stop, the few in its buffer run out and M4 keeps the beam off at rest
            } else if self.xtool {
                self.send_line("M22 S1");
            } else if self.marlin {
                // no feed hold: the lines stop and its buffer runs out; the beam goes off so a pause after a dot's or
                // a pierce's switch-on doesn't burn one spot (M107: a laser on the fan pin)
                self.send_lines(&["M5".into(), "M107".into()]);
                self.side_oks += 2;
            } else {
                self.realtime(grbl::HOLD);
            }
            self.streamer.pause();
            self.job = Job::Paused;
            self.note("Paused");
        }
    }
    pub fn resume(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &mut self.galvo {
            return g.resume();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &mut self.k40 {
            return k.resume();
        }
        if self.is_m1() {
            self.alert = Some("Resume the M1 with its own button".into());
            return;
        }
        if let Some(r) = &mut self.ruida {
            return r.resume();
        }
        if self.job == Job::Paused {
            if self.f1 {
            } else if self.xtool {
                self.send_line("M22 S2");
            } else if self.marlin {
                if !self.last_on.is_empty() {
                    let on = self.last_on.clone();
                    self.send_line(&on);
                    self.side_oks += 1;
                }
            } else {
                self.realtime(grbl::RESUME);
            }
            self.streamer.start();
            self.job = Job::Running;
            self.note("Resumed");
        }
    }
    /// Stop everything: hold, then reset (the beam goes off, the buffer is dropped); the job can't continue.
    pub fn stop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &mut self.galvo {
            return g.stop();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &mut self.k40 {
            return k.stop();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(m) = &mut self.m1 {
            return m.stop();
        }
        if let Some(r) = &mut self.ruida {
            return r.stop();
        }
        if self.f1 {
            // BeamInk's stop: the beam-off line, then the firmware's own stop on its web port
            (self.held, self.xq, self.x_fly) = (VecDeque::new(), VecDeque::new(), VecDeque::new());
            self.send_line("G0 S0");
            #[cfg(not(target_arch = "wasm32"))]
            f1_http_stop(self.f1_host.clone());
        } else if self.xtool {
            // the S1's cancel (it answers "M108 ok", then M222 S18 → S1); what it had queued may still run out
            self.held.clear();
            self.xq.clear();
            self.send_line("M108");
        } else if self.marlin {
            // quick stop (immediate with Marlin's emergency parser): the planner's moves are dropped, then the beam off
            self.held.clear();
            self.send_lines(&["M410".into(), "M5".into(), "M107".into()]); // M107: a laser on the fan pin goes off too
        } else {
            self.realtime(grbl::HOLD);
            self.realtime(grbl::RESET);
        }
        self.streamer.stop();
        if self.job != Job::None {
            self.job = Job::None;
            self.note("Stopped");
        }
    }
    pub fn progress(&self) -> (usize, usize) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &self.galvo {
            return (g.sent, g.total);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &self.k40 {
            return (k.sent, k.total);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(m) = &self.m1 {
            return ((m.phase == Phase::Done) as usize, 1);
        }
        if let Some(r) = &self.ruida {
            return (r.acked, r.total);
        }
        if self.xtool {
            return (self.x_acked, self.x_total);
        }
        (self.streamer.acked(), self.streamer.total())
    }
    pub fn job_error(&self) -> &str {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &self.galvo {
            return &g.error;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &self.k40 {
            return &k.error;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(m) = &self.m1 {
            return &m.error;
        }
        if let Some(r) = &self.ruida {
            return &r.error;
        }
        self.streamer.error()
    }
    pub fn job_time(&self) -> f64 {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &self.galvo {
            return g.job_seconds;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &self.k40 {
            return k.job_seconds;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(m) = &self.m1 {
            return m.job_seconds;
        }
        if let Some(r) = &self.ruida {
            return r.job_seconds;
        }
        self.job_seconds
    }

    /// machine position, mm
    pub fn pos_mm(&self) -> (f64, f64) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &self.galvo {
            let p = g.pos_mm(self.bed);
            return (p.x, p.y);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &self.k40 {
            let p = k.pos_mm();
            return (p.x, p.y);
        }
        if let Some(r) = &self.ruida {
            let p = r.pos_mm();
            return (p.x, p.y);
        }
        // reports are in mm, the simulator's too (gcarve's GrblSim reports like a real controller)
        let k = 1.0;
        let p = self.status.wpos();
        let p = if self.swap { gcarve_core::gcode::Vec3 { x: p.y, y: p.x, z: p.z } } else { p };
        (self.mirror[0].map_or(p.x * k, |w| w - p.x * k), self.mirror[1].map_or(p.y * k, |h| h - p.y * k))
    }

    /// Every frame: move the simulator's clock, read what came in, answer it, feed the streamer, poll the status.
    pub fn tick(&mut self, now: f64, dt: f64) {
        #[cfg(not(target_arch = "wasm32"))]
        keep_awake(matches!(self.job_state(), Job::Running | Job::Paused));
        #[cfg(target_arch = "wasm32")]
        if self.web_opening {
            use gcarve_ui::webserial::State;
            match gcarve_ui::webserial::state() {
                State::Open(desc) => {
                    self.web_opening = false;
                    match self.web_ruida.take() {
                        Some(opts) => match Ruida::with_link(Box::new(crate::ruida_link::WebSerialStream::new()), &format!("{desc} (Web Serial)"), opts) {
                            Ok(r) => {
                                self.ruida = Some(r);
                                self.alert = Some(format!("Talking to the Ruida on {desc}…"));
                            }
                            Err(e) => {
                                self.note(&e);
                                self.alert = Some(e);
                            }
                        },
                        None => {
                            self.alert = Some(format!("Connected to {desc}"));
                            self.attach(Box::new(WebSerial { desc }), false);
                            self.web_age = 0.0;
                        }
                    }
                }
                State::Cancelled => {
                    self.web_opening = false;
                    self.note("No port picked");
                    self.alert = Some("No port picked".into());
                }
                State::Failed(e) => {
                    self.web_opening = false;
                    self.note(&format!("Couldn't open the port: {e}"));
                    self.alert = Some(format!("Couldn't open the port: {e}. Another tab or program (GBurn, LightBurn) may have it: close them and try again"));
                }
                State::Opening => {}
            }
        }
        #[cfg(target_arch = "wasm32")]
        if self.link.is_some() && !self.is_ruida() {
            self.web_age += dt;
            if let gcarve_ui::webserial::State::Failed(e) = gcarve_ui::webserial::state() {
                // lost before the board said anything: it restarted as its port opened; open it again once, no chooser
                if self.web_age < 6.0 && !self.status.valid && !self.web_retried && gcarve_ui::webserial::reopen() {
                    self.note(&format!("The port went away right after opening ({e}): the board probably restarted; opening it again"));
                    self.alert = Some("The laser's board restarted when its port opened: connecting again…".into());
                    self.link = None;
                    self.status = MachineStatus::default();
                    self.web_retried = true;
                    self.web_opening = true;
                    return;
                }
                // say which kind of loss it was: the advice differs, and a problem report can then be told apart
                let why = if self.web_retried && !self.status.valid {
                    " It went away again after reopening: the board keeps restarting. Try another cable or USB socket, or the desktop app"
                } else if !self.status.valid {
                    " The laser never answered: check it is switched on, that this is its port, and the baud rate in Machine Settings"
                } else {
                    ""
                };
                self.note(&format!("Lost the port after {:.0} s: {e}", self.web_age));
                self.alert = Some(format!("Lost the port: {}.{why}", e.trim_end_matches('.')));
                self.disconnect();
                return;
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(g) = &mut self.galvo {
            self.job_started = now; // frames started from here use the app clock
            let was = g.phase;
            g.tick(now);
            // the set-up's end and a failure reach the status bar (the owner saw Idle in the panel only)
            if was == GPhase::Init && g.phase == GPhase::Idle {
                self.alert = Some("Galvo ready".into());
            } else if was != GPhase::Failed && g.phase == GPhase::Failed {
                self.alert = Some(format!("Galvo failed: {}", g.error));
            }
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = &mut self.k40 {
            let was = k.error.clone();
            k.tick(now);
            if !k.error.is_empty() && k.error != was {
                self.alert = Some(k.error.clone());
            }
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(m) = &mut self.m1 {
            let (was, waiting) = (m.error.clone(), m.state_text().starts_with("Press"));
            m.tick(now);
            if m.dead {
                // the first status answer never came, or wasn't an M1's: no link
                self.alert =
                    Some(format!("{}. Newer M1 firmware (40.18 and later) doesn't take jobs this way; over USB its address is usually 201.234.3.1 (Machine Settings › GRBL network address)", m.error));
                self.m1 = None;
                return;
            }
            if !m.error.is_empty() && m.error != was {
                self.alert = Some(m.error.clone());
            } else if !waiting && m.state_text().starts_with("Press") {
                self.alert = Some("The job is on the M1: press its button when it lights blue".into());
            }
            return;
        }
        if let Some(r) = &mut self.ruida {
            // a failure reaches the status bar (it only went to the Laser tab's log)
            let was = r.error.clone();
            r.tick(now);
            if !r.error.is_empty() && r.error != was {
                self.alert = Some(r.error.clone());
            }
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(t) = self.reopen_in.as_mut() {
            *t -= dt;
            if *t <= 0.0 {
                self.reopen_in = None;
                if let Some((path, baud)) = self.serial.clone() {
                    match self.connect_serial(&path, baud) {
                        Ok(()) => self.link_age = 10.0, // only one try: a second loss is reported
                        Err(e) => self.alert = Some(format!("Couldn't open {path} again after the board restarted: {e}")),
                    }
                }
            }
            return;
        }
        let Some(link) = &mut self.link else { return };
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.link_age += dt;
        }
        if let Some(e) = link.error() {
            #[cfg(not(target_arch = "wasm32"))]
            if self.serial.is_some() && self.link_age < 6.0 && !self.status.valid && !matches!(self.job, Job::Running | Job::Paused) {
                self.note(&format!("The port went away right after opening ({e}): the board probably restarted; opening it again"));
                self.alert = Some("The laser's board restarted when its port opened: connecting again…".into());
                let keep = self.serial.clone();
                self.disconnect();
                self.serial = keep;
                self.reopen_in = Some(2.0);
                return;
            }
            self.note(&format!("Lost the laser: {e}"));
            // mid-job: Preview's slider sits where the lines stopped, so Start here picks it up after a reconnect
            let resume = if matches!(self.job, Job::Running | Job::Paused) {
                ". The job stopped part way: reconnect, then Preview › ··· › Start here (the slider is where the lines stopped: move it back a little, the controller had some queued)"
            } else {
                ""
            };
            // Windows powers idle USB ports down (USB selective suspend), which drops GRBL jobs part way
            let win = if cfg!(windows) {
                " If this keeps happening on Windows: Control Panel › Power Options › Change plan settings › Advanced › USB settings › USB selective suspend: Disabled, and use a short cable straight into the computer."
            } else {
                ""
            };
            self.alert = Some(format!("Lost the laser: {e}{resume}{win}"));
            self.disconnect();
            return;
        }
        let Some(link) = &mut self.link else { return };
        link.tick(dt);
        let bytes = link.recv();
        if !bytes.is_empty() {
            self.rx.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(i) = self.rx.find('\n') {
                let line = self.rx[..i].trim_end_matches('\r').trim().to_string();
                self.rx.drain(..=i);
                self.on_line(&line);
            }
        }
        if self.job == Job::Paused && self.xtool {
            self.x_ok_at = now - self.job_started; // a pause isn't a silence: the 5 s wait starts again at Resume
        }
        if self.job == Job::Running && self.xtool {
            self.job_seconds = now - self.job_started;
            // the S1: one line at a time, the next after its ok; the F1: GRBL's 127-character count
            let room = if self.f1 { 127 } else { 0 };
            while let Some(line) = self.xq.front() {
                if !self.x_fly.is_empty() && self.x_fly.iter().sum::<usize>() + line.len() + 1 > room {
                    break;
                }
                let line = self.xq.pop_front().unwrap_or_default();
                if let Some(l) = &mut self.link {
                    l.send(format!("{line}\n").as_bytes());
                }
                self.x_fly.push_back(line.len() + 1);
                self.sent += 1;
            }
            if self.xq.is_empty() && self.x_acked >= self.x_total {
                self.job = Job::Done;
                self.note(&format!("Job done in {:.0} s", self.job_seconds));
            } else if self.f1 && !self.x_fly.is_empty() && self.job_seconds - self.x_ok_at > 5.0 {
                self.xq.clear();
                self.job = Job::Failed;
                self.alert = Some("The xTool F1 stopped answering (no ok in 5 s): job stopped. Is it on firmware before 40.51?".into());
                self.note("Job stopped: no answer");
            }
        } else if self.job == Job::Running {
            self.job_seconds = now - self.job_started;
            let chunk = self.streamer.next_chunk();
            self.held.extend(chunk.lines().map(|l| format!("{l}\n")));
            // buffered: everything the streamer allows; synchronous: one line, only when the last one was answered
            while let Some(line) = self.held.front() {
                if self.sync && self.sent > self.streamer.acked() {
                    break;
                }
                if let Some(l) = &mut self.link {
                    l.send(line.as_bytes());
                }
                // the M code's number alone (M410 isn't M4; the streamer drops spaces: "M4IS0")
                let m: String = line.strip_prefix('M').map_or(String::new(), |r| r.chars().take_while(char::is_ascii_digit).collect());
                match m.as_str() {
                    "3" | "4" | "106" => self.last_on = line.trim_end().to_string(),
                    "5" | "107" => self.last_on.clear(),
                    _ => {}
                }
                self.held.pop_front();
                self.sent += 1;
                if self.sync {
                    break;
                }
            }
            if self.streamer.done() {
                self.job = Job::Done;
                self.note(&format!("Job done in {:.0} s", self.job_seconds));
            } else if !self.streamer.running() && !self.streamer.error().is_empty() {
                self.job = Job::Failed;
                let e = self.streamer.error().to_string();
                self.note(&format!("Job stopped: {e}"));
            }
        }
        if self.f1 {
            // nothing to poll on 8780
        } else if self.xtool {
            if self.job != Job::Running && self.job != Job::Paused && now - self.last_poll > 0.5 {
                self.last_poll = now;
                if let Some(l) = &mut self.link {
                    l.send(b"?\n");
                }
            }
        } else if self.marlin {
            // M114 goes through Marlin's queue (and its ok would count for the job): only between jobs
            if self.job != Job::Running && self.job != Job::Paused && now - self.last_poll > 0.5 {
                self.last_poll = now;
                if let Some(l) = &mut self.link {
                    l.send(b"M114\n");
                }
            }
        } else if now - self.last_poll > 0.2 {
            self.last_poll = now;
            self.realtime(grbl::STATUS);
        }
    }

    fn on_line(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        if grbl::parse_status(line, &mut self.status, 1.0) {
            return; // status reports are not logged (5 a second)
        }
        if self.xtool {
            if let Some(v) = line.strip_prefix("MPos:") {
                let n: Vec<f64> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if n.len() >= 2 {
                    (self.status.mpos.x, self.status.mpos.y) = (n[0], n[1]);
                    self.status.valid = true;
                    if !matches!(self.job, Job::Running | Job::Paused) {
                        self.status.state = "Idle".into();
                    }
                }
                return;
            }
            if line == "ok" {
                if matches!(self.job, Job::Running | Job::Paused) && self.x_acked < self.sent {
                    self.x_acked += 1;
                    self.x_fly.pop_front();
                    self.x_ok_at = self.job_seconds;
                }
                return; // a job line's ok, or the poll's own
            }
            if line.starts_with("error") && matches!(self.job, Job::Running | Job::Paused) && self.x_acked < self.sent {
                self.x_acked += 1;
                self.x_fly.pop_front();
                self.x_ok_at = self.job_seconds; // answered all the same: note it and go on, like GRBL's streamer
                self.alert = Some(format!("xTool: {line}"));
                return self.note(line);
            }
            if let Some(n) = line.strip_prefix("M222 S").and_then(|v| v.trim().parse::<u32>().ok()) {
                self.status.state = xtool_state(n).into();
                if n == 7 || n == 9 {
                    self.alert = Some(format!("xTool: {}", xtool_state(n)));
                }
                return;
            }
            if line.starts_with("M53") {
                if line.contains("B1") {
                    self.alert = Some("xTool: the lid is open (close it to jog, frame or burn)".into());
                } else if line.contains("F1") {
                    self.alert = Some("xTool: FLAME ALARM".into());
                }
                return self.note(line);
            }
            if line.starts_with("ALARM:cancel") {
                self.alert = Some("The xTool cancelled the job: is the lid shut and the base / riser switch in place?".into());
                if matches!(self.job, Job::Running | Job::Paused) {
                    self.xq.clear();
                    self.job = Job::Failed;
                }
                return self.note(line);
            }
            if line.starts_with('M') || line.starts_with('[') || line.starts_with("xTool") {
                return self.note(line); // the machine's own messages are not acks
            }
        }
        if self.marlin {
            // M114: "X:10.00 Y:20.00 Z:0.00 E:0.00 Count X:…": the position (mm), no state word
            if let Some(p) = marlin_pos(line) {
                (self.status.mpos.x, self.status.mpos.y) = p;
                self.status.valid = true;
                self.status.state = if self.job == Job::Running { "Run".into() } else { "Idle".into() };
                return;
            }
            if line == "ok" && self.side_oks > 0 {
                self.side_oks -= 1; // a pause / resume line's own ok
                return;
            }
            if line == "ok" && !matches!(self.job, Job::Running | Job::Paused) {
                return; // the poll's own ok (a job line answered during a pause still counts for the job)
            }
            if line.starts_with("echo:busy") {
                return;
            }
        }
        if let Some((k, v)) = grbl::parse_setting(line) {
            if k == 30 {
                self.s_max = v;
            }
            // $32 = 0 (CNC mode) waits at every corner with the beam on: dark dots at corners and starts
            if k == 32 && v == 0.0 && !self.marlin {
                self.alert = Some("Laser mode is off on the controller ($32=0): corners and starts will burn dark dots. Laser › Controller › GRBL Settings: set $32 to 1".into());
            }
            match self.settings.iter_mut().find(|(n, _)| *n == k) {
                Some(s) => s.1 = v,
                None => self.settings.push((k, v)),
            }
            return;
        }
        if !self.streamer.on_response(line) || line.starts_with("error") {
            let text = if let Some(n) = line.strip_prefix("error:") {
                format!("{line} {}", grbl::error_text(n.trim().parse().unwrap_or(0)))
            } else if let Some(n) = line.strip_prefix("ALARM:") {
                format!("{line} {}", grbl::alarm_text(n.trim().parse().unwrap_or(0)))
            } else {
                line.to_string()
            };
            self.note(&text);
        }
    }
}

impl Laser {
    /// BeamInk's session round a few lines (jog, home, frame): `$L`, the source, … `M6 P1`, `$P`
    fn f1_session(&self, body: Vec<String>) -> Vec<String> {
        let src = if self.f1_ir { "M114 S2" } else { "M114 S1" };
        let mut v: Vec<String> = ["$L", "G0 F180000", "M4 S0", "G1 F180000", src, "G21", "G90"].map(String::from).to_vec();
        v.extend(body);
        v.extend(["G90", "G0 S0", "G0 F180000", "G1 F180000", "M6 P1", "$P"].map(String::from));
        v
    }
}

/// The F1's own stop on its web port (POST, then GET: BeamInk tries both); off the UI thread, the result logged nowhere
/// ponytail: fire and forget; the `G0 S0` already went down the job's socket
#[cfg(not(target_arch = "wasm32"))]
fn f1_http_stop(host: String) {
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for method in ["POST", "GET"] {
            let Ok(mut c) = std::net::TcpStream::connect((host.as_str(), 8080)) else { return };
            let _ = c.set_read_timeout(Some(std::time::Duration::from_secs(3)));
            let len = if method == "POST" { "Content-Length: 0\r\n" } else { "" };
            if c.write_all(format!("{method} /processing/stop HTTP/1.1\r\nHost: {host}\r\n{len}Connection: close\r\n\r\n").as_bytes()).is_err() {
                continue;
            }
            let mut head = [0u8; 12];
            if c.read(&mut head).is_ok_and(|n| n >= 12 && head[9] == b'2') {
                return; // "HTTP/1.1 2xx"
            }
        }
    });
}

/// An S1's M222 state number in words (ha-xtool's table)
fn xtool_state(n: u32) -> &'static str {
    match n {
        1 | 3 => "Idle",
        7 => "Lid opened mid-job",
        9 => "Fire alarm",
        10 => "Measuring",
        11 | 12 => "Framing",
        13 | 14 => "Run",
        15 => "Hold",
        18 => "Cancelling",
        19 => "Done",
        22 => "Homing error",
        24 => "Press the machine's button",
        _ => "Busy",
    }
}

/// Marlin's M114 answer → (x, y) mm
fn marlin_pos(line: &str) -> Option<(f64, f64)> {
    let head = line.split(" Count").next()?;
    let get = |k: &str| head.split_whitespace().find_map(|w| w.strip_prefix(k)?.parse::<f64>().ok());
    Some((get("X:")?, get("Y:")?))
}

#[cfg(test)]
mod tests {
    /// a port that takes `room` bytes a call, then says it timed out / took nothing, until given more room
    struct Busy {
        got: Vec<u8>,
        room: usize,
        kind: Option<std::io::ErrorKind>,
    }
    impl std::io::Write for Busy {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if self.room == 0 {
                return self.kind.map_or(Ok(0), |k| Err(k.into()));
            }
            let k = b.len().min(self.room);
            self.room -= k;
            self.got.extend_from_slice(&b[..k]);
            Ok(k)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_port_lost_right_after_opening_is_opened_again_once() {
        /// a board that drops off USB as soon as it is opened
        struct Gone;
        impl Link for Gone {
            fn send(&mut self, _: &[u8]) {}
            fn recv(&mut self) -> Vec<u8> {
                Vec::new()
            }
            fn error(&mut self) -> Option<String> {
                Some("Access denied".into())
            }
            fn name(&self) -> String {
                "COM4".into()
            }
        }
        let mut l = Laser::default();
        l.attach(Box::new(Gone), false);
        l.serial = Some(("/dev/no-such-port".into(), 115200));
        l.tick(0.0, 0.1);
        assert!(!l.connected() && l.opening() && l.alert.as_deref().unwrap_or("").contains("restarted"), "{:?}", l.alert);
        l.tick(1.0, 2.5); // the reopen: this port can't open, so it says so and stops trying
        assert!(!l.opening() && l.alert.as_deref().unwrap_or("").starts_with("Couldn't open /dev/no-such-port again"), "{:?}", l.alert);
        // a link lost later, or after the board has answered, is reported as lost
        l.attach(Box::new(Gone), false);
        l.serial = Some(("/dev/no-such-port".into(), 115200));
        l.link_age = 7.0;
        l.tick(2.0, 0.1);
        assert!(!l.opening() && l.alert.as_deref().unwrap_or("").starts_with("Lost the laser: Access denied"));
    }

    #[test]
    fn a_busy_port_keeps_the_bytes_and_only_a_real_error_loses_the_link() {
        for kind in [None, Some(std::io::ErrorKind::TimedOut), Some(std::io::ErrorKind::WouldBlock)] {
            let (mut w, mut out, mut err) = (Busy { got: vec![], room: 3, kind }, b"G1 X10\n".to_vec(), None);
            pump(&mut w, &mut out, &mut err);
            assert_eq!((w.got.as_slice(), out.as_slice(), &err), (b"G1 ".as_slice(), b"X10\n".as_slice(), &None), "{kind:?}");
            w.room = 99;
            pump(&mut w, &mut out, &mut err);
            assert_eq!((w.got.as_slice(), out.is_empty(), err), (b"G1 X10\n".as_slice(), true, None));
        }
        let (mut w, mut out, mut err) = (Busy { got: vec![], room: 0, kind: Some(std::io::ErrorKind::PermissionDenied) }, b"x".to_vec(), None);
        pump(&mut w, &mut out, &mut err);
        assert!(err.is_some() && out.is_empty());
    }

    use super::*;

    #[test]
    fn the_ruidas_motion_settings_are_offered_only_when_plausible() {
        struct Nil;
        impl crate::ruida_link::Datagram for Nil {
            fn send(&mut self, _: &[u8]) {}
            fn recv(&mut self) -> Option<Vec<u8>> {
                None
            }
        }
        let mut l = Laser { ruida: Some(Ruida::with_link(Box::new(Nil), "test", Default::default()).unwrap()), ..Default::default() };
        let set = |l: &mut Laser, v: [i64; 5]| l.ruida.as_mut().unwrap().settings = gburn_core::ruida::MOTION_SETTINGS.iter().copied().zip(v).collect();
        set(&mut l, [500_000, 400_000, 3_000_000, 2_000_000, 0]);
        // no cutting acceleration: the lower axis one
        assert_eq!(l.controller_motion(), Some((2000.0, 3000.0, [500.0, 400.0])));
        set(&mut l, [500_000, 400_000, 3_000_000, 2_000_000, 1_500_000]);
        assert_eq!(l.controller_motion().map(|m| m.0), Some(1500.0));
        set(&mut l, [500_000_000, 400_000, 3_000_000, 2_000_000, 0]); // 500 m/s: a wrong address, not offered
        assert_eq!(l.controller_motion(), None);
    }

    /// Marlin: no reset or $$, the position from M114, one line at a time, G91 G0 jogs, M410 to stop
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    /// xTool F1 (old firmware): port 8780 by default is the caller's; no reset / `?` / `$$`; lines whole, several in
    /// flight within 127 characters; jog / frame wrapped in `$L` … `$P`; Stop = `G0 S0`; silence fails the job
    fn an_xtool_f1_streams_grbl_like_lines_in_its_session() {
        use std::io::{BufRead, Write};
        let lis = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = lis.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen2 = seen.clone();
        let mute = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mute2 = mute.clone();
        std::thread::spawn(move || {
            let (c, _) = lis.accept().unwrap();
            let mut w = c.try_clone().unwrap();
            for line in std::io::BufReader::new(c).split(b'\n').map_while(Result::ok) {
                seen2.lock().unwrap().push(String::from_utf8_lossy(&line).trim().to_string());
                if !mute2.load(std::sync::atomic::Ordering::SeqCst) {
                    let _ = w.write_all(b"ok\n");
                }
            }
        });
        let mut l = Laser { xtool: true, f1: true, f1_ir: true, bed: [115.0, 115.0], ..Laser::default() };
        l.connect_tcp(&format!("127.0.0.1:{port}")).unwrap();
        assert_eq!(l.f1_host, "127.0.0.1");
        let mut t = 0.0;
        let run = |l: &mut Laser, t: &mut f64, n: usize| {
            for _ in 0..n {
                *t += 0.05;
                l.tick(*t, 0.05);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        };
        run(&mut l, &mut t, 5);
        assert!(!l.is_grbl() && l.is_gcode() && l.status.state == "Idle");
        let mut job: Vec<String> = ["$L", "G0 F180000", "M4 S0", "G1 F180000", "M114 S2", "G21", "G90"].map(String::from).to_vec();
        job.extend((0..40).map(|i| format!("G1 X{i} Y5 S200 F600")));
        job.extend(["G0 S0", "M6 P1", "$P"].map(String::from));
        l.start(&job, t);
        let mut most = 0;
        for _ in 0..300 {
            run(&mut l, &mut t, 1);
            most = most.max(l.sent - l.x_acked);
            assert!(l.x_fly.iter().sum::<usize>() <= 127);
            if l.job == Job::Done {
                break;
            }
        }
        assert_eq!(l.job, Job::Done, "{:?}", l.log);
        assert!(most > 1, "several lines in flight");
        l.home();
        l.jog(3.0, 0.0, 1200.0);
        l.frame_path(&[gburn_core::pt(0.0, 0.0), gburn_core::pt(10.0, 0.0)], 6000.0, 0.0);
        run(&mut l, &mut t, 10);
        mute.store(true, std::sync::atomic::Ordering::SeqCst);
        l.start(&job, t);
        run(&mut l, &mut t, 120);
        assert_eq!(l.job, Job::Failed, "silence stops the job");
        assert!(l.alert.as_deref().unwrap_or("").contains("stopped answering"));
        l.f1_host = "0.0.0.0".into(); // the HTTP stop goes nowhere in a test
        l.stop();
        run(&mut l, &mut t, 10);
        let seen = seen.lock().unwrap().clone();
        assert!(!seen.iter().any(|s| s == "?" || s == "$$" || s.contains('\u{18}')), "{seen:?}");
        assert_eq!(seen.iter().filter(|s| *s == "G1 X39 Y5 S200 F600").count(), 1, "every line once, whole");
        assert!(seen.windows(3).any(|w| w == ["G90", "G0 X57.500 Y57.500", "G90"]), "home = the centre inside a session: {seen:?}");
        assert!(seen.windows(3).any(|w| w == ["G91", "G0 X3.000 Y0.000", "G90"]));
        assert!(seen.windows(2).any(|w| w == ["M114 S2", "G21"]) && seen.iter().any(|s| s == "G1 X0.000 Y0.000 S0 F6000"));
        assert_eq!(seen.last().map(String::as_str), Some("G0 S0"), "Stop's beam-off line");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    /// xTool S1: no reset or $ commands, `?` → MPos, every line sent whole (spaces kept, M6 not a tool change), one at
    /// a time; M222 states and the lid reach the status; jog / frame are S0 moves; Stop is M108
    fn an_xtool_s1_streams_its_own_dialect_line_by_line() {
        use std::io::{BufRead, Write};
        let lis = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = lis.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            let (c, _) = lis.accept().unwrap();
            let mut w = c.try_clone().unwrap();
            for line in std::io::BufReader::new(c).split(b'\n').map_while(Result::ok) {
                let l = String::from_utf8_lossy(&line).trim().to_string();
                seen2.lock().unwrap().push(l.clone());
                let reply = match l.as_str() {
                    "?" => "MPos:12.5,30.0,0.0,0.0\nok\n".to_string(),
                    "M108" => "M108 ok\nM222 S18\nM222 S1\n".to_string(),
                    "M4" => "M15 A1 S1\nok\n".to_string(), // an unasked line before the ok: not an ack
                    _ => "ok\n".to_string(),
                };
                let _ = w.write_all(reply.as_bytes());
            }
        });
        let mut l = Laser { xtool: true, ..Laser::default() };
        l.connect_tcp(&format!("127.0.0.1:{port}")).unwrap();
        let mut t = 0.0;
        let run = |l: &mut Laser, t: &mut f64, n: usize| {
            for _ in 0..n {
                *t += 0.05;
                l.tick(*t, 0.05);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        };
        run(&mut l, &mut t, 30);
        assert_eq!(l.pos_mm(), (12.5, 30.0), "{:?}", l.log);
        assert!(!l.is_grbl() && l.is_gcode());
        let job: Vec<String> = ["G21", "G90", "M4", "M15 S1", "G1 X5 Y5 S200 F600", "M15 S0", "M5", "M6", "M7 S0 N0 D10"].iter().map(|s| s.to_string()).collect();
        l.start(&job, t);
        for _ in 0..300 {
            run(&mut l, &mut t, 1);
            assert!(l.sent <= l.x_acked + 1, "one line in flight");
            if l.job == Job::Done {
                break;
            }
        }
        assert_eq!(l.job, Job::Done, "{:?}", l.log);
        assert_eq!(l.progress(), (9, 9));
        l.frame_path(&[gburn_core::pt(0.0, 0.0), gburn_core::pt(10.0, 0.0)], 6000.0, 0.0);
        l.jog(3.0, 0.0, 1200.0);
        l.job = Job::Running; // as if mid-job
        l.stop();
        run(&mut l, &mut t, 20);
        let seen = seen.lock().unwrap().clone();
        assert!(!seen.iter().any(|s| s.starts_with('$') || s.contains('\u{18}')), "no GRBL reset / settings: {seen:?}");
        let at = seen.iter().position(|s| s == "M4").unwrap();
        assert_eq!(&seen[at..at + 6], ["M4", "M15 S1", "G1 X5 Y5 S200 F600", "M15 S0", "M5", "M6"], "whole lines, M6 sent like any other");
        assert!(seen.windows(4).any(|w| w == ["M17 S1", "M106 S1", "G21 G90", "G1 X0.000 Y0.000 S0 F6000"]), "{seen:?}");
        assert!(seen.windows(3).any(|w| w == ["G91", "G1 X3.000 Y0.000 S0 F1200", "G90"]), "{seen:?}");
        assert!(seen.contains(&"M108".to_string()) && l.status.state == "Idle", "{}", l.status.state);
    }

    #[test]
    fn a_marlin_laser_streams_line_by_line_and_reports_its_position() {
        use std::io::{BufRead, Write};
        assert_eq!(marlin_pos("X:10.50 Y:-2.00 Z:0.00 E:0.00 Count X:840 Y:-160 Z:0"), Some((10.5, -2.0)));
        assert_eq!(marlin_pos("ok"), None);
        let lis = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = lis.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            let (c, _) = lis.accept().unwrap();
            let mut w = c.try_clone().unwrap();
            for line in std::io::BufReader::new(c).split(b'\n').map_while(Result::ok) {
                let l = String::from_utf8_lossy(&line).trim().to_string();
                seen2.lock().unwrap().push(l.clone());
                let reply = if l == "M114" { "X:1.00 Y:2.00 Z:0.00 E:0.00 Count X:80 Y:160 Z:0\nok\n".to_string() } else { "ok\n".to_string() };
                let _ = w.write_all(reply.as_bytes());
            }
        });
        let mut l = Laser { marlin: true, ..Laser::default() };
        l.connect_tcp(&format!("127.0.0.1:{port}")).unwrap();
        let mut t = 0.0;
        let run = |l: &mut Laser, t: &mut f64, n: usize| {
            for _ in 0..n {
                *t += 0.05;
                l.tick(*t, 0.05);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        };
        run(&mut l, &mut t, 30);
        assert_eq!(l.pos_mm(), (1.0, 2.0), "{:?}", l.log);
        assert!(l.status.valid && !l.is_grbl() && l.is_gcode());
        l.sync = true;
        let job: Vec<String> = ["G21", "G90", "M4 I S0", "G1 X5 Y5 F600 S200", "M5"].iter().map(|s| s.to_string()).collect();
        l.start(&job, t);
        for _ in 0..200 {
            run(&mut l, &mut t, 1);
            assert!(l.sent <= l.streamer.acked() + 1);
            if l.job == Job::Done {
                break;
            }
        }
        assert_eq!(l.job, Job::Done, "{:?}", l.log);
        // pause mid-job: the beam goes off (M5 / M107); resume switches it back on and the job still finishes
        let job: Vec<String> = ["M4 I S0", "G1 X6 F600 S200", "G1 X7 S200", "G1 X8 S200", "G1 X9 S200", "M5"].iter().map(|s| s.to_string()).collect();
        l.start(&job, t);
        while l.streamer.acked() < 2 {
            run(&mut l, &mut t, 1);
        }
        l.pause();
        run(&mut l, &mut t, 10);
        l.resume();
        for _ in 0..200 {
            run(&mut l, &mut t, 1);
            if l.job == Job::Done {
                break;
            }
        }
        assert_eq!(l.job, Job::Done, "{:?}", l.log);
        {
            let seen = seen.lock().unwrap();
            let p = seen.iter().position(|s| s == "M107").unwrap_or_else(|| panic!("{seen:?}"));
            assert_eq!(seen[p - 1], "M5");
            assert!(seen[p + 1..].iter().any(|s| s == "M4IS0"), "switched back on: {seen:?}");
        }
        l.jog(3.0, 0.0, 1200.0);
        l.stop();
        run(&mut l, &mut t, 10);
        let seen = seen.lock().unwrap().clone();
        assert!(!seen.iter().any(|s| s == "$$" || s.contains('\u{18}')), "no GRBL reset / settings: {seen:?}");
        assert!(seen.windows(3).any(|w| w[0] == "G91" && w[1] == "G0 X3.000 Y0.000 F1200" && w[2] == "G90"), "{seen:?}");
        assert!(seen.windows(2).any(|w| w[0] == "M410" && w[1] == "M5"), "{seen:?}");
        let job_at = seen.iter().position(|s| s == "M4IS0").unwrap_or_else(|| panic!("{seen:?}")); // the streamer drops spaces; Marlin reads it
        assert!(!seen[job_at..seen.iter().position(|s| s == "M5").unwrap()].iter().any(|s| s == "M114"), "no polls mid-job");
    }

    /// network GRBL: the app's TCP link talks to a stand-in controller that answers "ok" per line
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_network_grbl_streams_a_job_over_tcp() {
        use std::io::{BufRead, Write};
        let lis = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = lis.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (c, _) = lis.accept().unwrap();
            let mut w = c.try_clone().unwrap();
            for line in std::io::BufReader::new(c).split(b'\n').map_while(Result::ok) {
                let l = String::from_utf8_lossy(&line).trim_matches(|ch: char| ch.is_control() || ch == '?').to_string();
                if !l.is_empty() {
                    let _ = w.write_all(if l == "$$" { b"$30=1000\r\nok\r\n".as_slice() } else { b"ok\r\n".as_slice() });
                }
            }
        });
        let mut l = Laser::default();
        l.connect_tcp(&format!("127.0.0.1:{port}")).unwrap();
        let job: Vec<String> = ["G21", "G90", "M4 S0", "G1 X5 Y5 F600 S300", "M5"].iter().map(|s| s.to_string()).collect();
        let mut t = 0.0;
        for _ in 0..50 {
            t += 0.02;
            l.tick(t, 0.02);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(l.s_max, 1000.0, "$$ answered over the network: {:?}", l.log);
        l.start(&job, t);
        for _ in 0..200 {
            t += 0.02;
            l.tick(t, 0.02);
            if l.job == Job::Done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(l.job, Job::Done, "{:?}", l.log);
        assert!(l.name().starts_with("GRBL at 127.0.0.1:"));
    }

    /// the simulated laser runs a small job end to end through the same code a real GRBL would see
    #[test]
    fn the_simulated_laser_runs_a_job_and_reports_where_it_is() {
        let mut l = Laser::default();
        l.connect_sim([300.0, 200.0], 100.0);
        let mut t = 0.0;
        fn step(l: &mut Laser, t: &mut f64, n: usize) {
            for _ in 0..n {
                *t += 0.05;
                l.tick(*t, 0.05);
            }
        }
        step(&mut l, &mut t, 20);
        assert_eq!(l.status.state, "Idle", "{:?}", l.log);
        assert!(!l.settings.is_empty(), "$$ answered: {:?}", l.settings); // ponytail: the CNC sim has no $30; a real GRBL does
        l.jog(10.0, 5.0, 3000.0);
        step(&mut l, &mut t, 60);
        let (x, y) = l.pos_mm();
        assert!((x - 10.0).abs() < 0.01 && (y - 5.0).abs() < 0.01, "{x} {y}");
        let job: Vec<String> = ["G21", "G90", "M4 S0", "G0 X20 Y20", "G1 X40 Y20 F3000 S500", "G1 X40 Y40", "M5", "G0 X0 Y0"].iter().map(|s| s.to_string()).collect();
        l.start(&job, t);
        assert_eq!(l.job, Job::Running);
        step(&mut l, &mut t, 200);
        assert_eq!(l.job, Job::Done, "{:?}", l.log);
        assert_eq!(l.progress(), (8, 8));
        let (x, y) = l.pos_mm();
        assert!(x.abs() < 0.01 && y.abs() < 0.01, "{x} {y}");
        // overrides and settings
        l.realtime(grbl::SPINDLE_MINUS_10);
        l.realtime(grbl::FEED_PLUS_10);
        step(&mut l, &mut t, 5);
        assert_eq!((l.status.feed_ov, l.status.spindle_ov), (110, 90));
        let n = l.settings().len();
        l.write_setting(130, 321.5);
        step(&mut l, &mut t, 5);
        l.read_settings();
        step(&mut l, &mut t, 10);
        assert!(l.settings().len() >= n.min(1) && l.settings().iter().filter(|(k, _)| *k == 130).count() <= 1, "{:?}", l.settings());
        assert!(l.log.iter().any(|s| s == "> $130=321.5"), "{:?}", l.log);
        // a machine with 0,0 at the right: +10 mm on the design is −10 on the machine, and the position reads mirrored
        let (x_before, _) = l.pos_mm();
        l.mirror = [Some(300.0), None];
        assert!((l.pos_mm().0 - (300.0 - x_before)).abs() < 1e-6);
        l.jog(10.0, 0.0, 3000.0);
        assert!(l.log.iter().any(|s| s.starts_with("> $J=G91 G21 X-10.000")), "{:?}", l.log.iter().rev().take(2).collect::<Vec<_>>());
        step(&mut l, &mut t, 60);
        l.mirror = [None, None];
        // X / Y swapped: a design X jog goes out as Y
        l.swap = true;
        l.jog(5.0, 0.0, 3000.0);
        assert!(l.log.iter().any(|s| s.starts_with("> $J=G91 G21 X0.000 Y5.000")), "{:?}", l.log.iter().rev().take(2).collect::<Vec<_>>());
        step(&mut l, &mut t, 60);
        l.swap = false;
        // synchronous transfer: never more than one line waiting for its ok, and the job still finishes
        l.sync = true;
        l.start(&job, t);
        for _ in 0..300 {
            t += 0.05;
            l.tick(t, 0.05);
            assert!(l.sent <= l.streamer.acked() + 1, "{} sent, {} acked", l.sent, l.streamer.acked());
        }
        assert_eq!(l.job, Job::Done, "{:?}", l.log);
        l.sync = false;
        // stop drops a job
        l.start(&job, t);
        l.stop();
        assert_eq!(l.job, Job::None);
        step(&mut l, &mut t, 5);
        l.disconnect();
        assert!(!l.connected());
    }
}
