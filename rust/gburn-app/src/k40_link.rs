//! A K40 (Lihuiyu M2 / M3 Nano board) over its CH341 USB chip (desktop, `nusb`), after MeerK40t's controller and
//! ch341 libusb driver (MIT). No threads: `tick` every frame polls the board's status byte and sends the next
//! 30-byte packet when it says OK, resends on a CRC refusal, and after a `FNSE-` packet waits until it reports the
//! work finished. Everything the board does is relative to where the head is, so the link keeps the position.
//! Safety: the beam only fires on a `D` inside a job's cut; jogs and Frame are rapid moves (no D possible); Stop
//! drops the queue and sends `I` ahead of it; power is the machine's own knob.

#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

use crate::ruida_link::Phase;
use gburn_core::k40::{self, Egv, Packet};
use gburn_core::Pt;
use std::collections::VecDeque;
use std::time::Duration;

/// The CH341's bulk pipe: write a packet, read the board's status byte.
pub trait Ch341 {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn status(&mut self) -> Result<u8, String>;
}

#[cfg(not(target_arch = "wasm32"))]
pub struct NusbCh341 {
    out: nusb::Endpoint<nusb::transfer::Bulk, nusb::transfer::Out>,
    inp: nusb::Endpoint<nusb::transfer::Bulk, nusb::transfer::In>,
    _iface: nusb::Interface,
    _dev: nusb::Device,
}

#[cfg(not(target_arch = "wasm32"))]
/// Is a K40 board (CH341 1a86:5512) on USB? Only looks.
pub fn k40_present() -> bool {
    use nusb::MaybeFuture;
    nusb::list_devices().wait().is_ok_and(|mut it| it.any(|d| d.vendor_id() == k40::USB_VID && d.product_id() == k40::USB_PID))
}

#[cfg(not(target_arch = "wasm32"))]
impl NusbCh341 {
    /// Open the board and put the CH341 in EPP 1.9 parallel mode (MeerK40t: vendor request 0xB1, value 0x0102).
    pub fn open() -> Result<(NusbCh341, u16), String> {
        use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient};
        use nusb::MaybeFuture;
        let info = nusb::list_devices()
            .wait()
            .map_err(|e| e.to_string())?
            .find(|d| d.vendor_id() == k40::USB_VID && d.product_id() == k40::USB_PID)
            .ok_or("no K40 board on USB (CH341 1a86:5512). Windows: it must use the WinUSB driver (Zadig), not the CH341PAR driver LaserDRW installs.")?;
        let dev = info.open().wait().map_err(|e| crate::laser::access_hint(format!("open: {e}")))?;
        let iface = dev.claim_interface(0).wait().map_err(|e| format!("claim: {e} (is LaserDRW, K40 Whisperer or MeerK40t holding it?)"))?;
        let t = Duration::from_millis(500);
        iface
            .control_out(ControlOut { control_type: ControlType::Vendor, recipient: Recipient::Device, request: 0xB1, value: 0x0102, index: 0, data: &[] }, t)
            .wait()
            .map_err(|e| format!("CH341 parallel mode: {e}"))?;
        let ver = iface
            .control_in(ControlIn { control_type: ControlType::Vendor, recipient: Recipient::Device, request: 0x5F, value: 0, index: 0, length: 2 }, t)
            .wait()
            .map_or(0, |b| b.first().copied().unwrap_or(0) as u16 | (b.get(1).copied().unwrap_or(0) as u16) << 8);
        let out = iface.endpoint::<nusb::transfer::Bulk, nusb::transfer::Out>(0x02).map_err(|e| format!("out endpoint: {e}"))?;
        let inp = iface.endpoint::<nusb::transfer::Bulk, nusb::transfer::In>(0x82).map_err(|e| format!("in endpoint: {e}"))?;
        Ok((NusbCh341 { out, inp, _iface: iface, _dev: dev }, ver))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Ch341 for NusbCh341 {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        let c = self.out.transfer_blocking(nusb::transfer::Buffer::from(bytes), Duration::from_millis(500));
        c.status.map_err(|e| format!("USB write: {e} (unplug the laser's USB and plug it back in, then Connect again)"))
    }
    fn status(&mut self) -> Result<u8, String> {
        // the CH341's status command (0xA0) on the bulk pipe, then its 6-byte answer: byte 1 is the board's
        let c = self.out.transfer_blocking(nusb::transfer::Buffer::from(&[0xA0u8][..]), Duration::from_millis(200));
        c.status.map_err(|e| format!("USB status: {e}"))?;
        let c = self.inp.transfer_blocking(nusb::transfer::Buffer::new(self.inp.max_packet_size()), Duration::from_millis(200));
        c.status.map_err(|e| format!("USB status: {e}"))?;
        c.buffer.get(1).copied().ok_or_else(|| "USB status: no answer".to_string())
    }
}

/// A pretend board: takes every packet (refuses one with a bad CRC once if asked), says FINISH a few polls after a
/// wait, records the packets' 30 command bytes.
#[derive(Default)]
pub struct MockK40 {
    pub written: Vec<Vec<u8>>,
    pub refuse_next: bool,
    last: u8,
    busy: u32,
}

impl Ch341 for MockK40 {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.refuse_next {
            self.refuse_next = false;
            self.last = k40::STATUS_ERROR;
            return Ok(());
        }
        self.written.push(bytes[2..32].to_vec());
        self.last = k40::STATUS_OK;
        self.busy = 3;
        Ok(())
    }
    fn status(&mut self) -> Result<u8, String> {
        if self.last == k40::STATUS_ERROR {
            return Ok(k40::STATUS_ERROR);
        }
        if self.busy > 0 {
            self.busy -= 1;
            return Ok(k40::STATUS_OK); // OK keeps bit 1 set: "still working"
        }
        Ok(k40::STATUS_FINISH)
    }
}

pub struct K40 {
    usb: Box<dyn Ch341>,
    pub phase: Phase,
    queue: VecDeque<Packet>,
    /// sent, waiting for the board's OK (tries, since when)
    pending: Option<(Packet, u32, f64)>,
    /// after a `-` packet: until the board reports the work done
    finishing: Option<f64>,
    /// the head, mils from the home corner (top left)
    pub at: (i64, i64),
    /// a Stop left the head somewhere unknown: Home before the next job
    pub lost: bool,
    pub bed: [f64; 2],
    pub sent: usize,
    pub total: usize,
    pub error: String,
    pub log: VecDeque<String>,
    pub job_seconds: f64,
    started: f64,
    in_job: bool,
}

impl K40 {
    pub fn new(usb: Box<dyn Ch341>, bed: [f64; 2], chip: u16) -> K40 {
        let mut k = K40 {
            usb,
            phase: Phase::Idle,
            queue: VecDeque::new(),
            pending: None,
            finishing: None,
            at: (0, 0),
            lost: false,
            bed,
            sent: 0,
            total: 0,
            error: String::new(),
            log: VecDeque::new(),
            job_seconds: 0.0,
            started: 0.0,
            in_job: false,
        };
        k.note(&format!("K40 board connected (CH341 version {chip}). The head is taken to be at home (top left): press Home if it was moved by hand"));
        k
    }
    fn note(&mut self, s: &str) {
        self.log.push_back(s.to_string());
        while self.log.len() > 2000 {
            self.log.pop_front();
        }
    }
    pub fn state_text(&self) -> &'static str {
        match self.phase {
            Phase::Idle if self.busy() => "Moving",
            Phase::Idle => "Idle",
            Phase::Sending | Phase::Running => "Run",
            Phase::Paused => "Hold",
            Phase::Done => "Done",
            Phase::Failed => "Failed",
        }
    }
    fn busy(&self) -> bool {
        !self.queue.is_empty() || self.pending.is_some() || self.finishing.is_some()
    }
    fn running(&self) -> bool {
        matches!(self.phase, Phase::Sending | Phase::Running | Phase::Paused)
    }
    fn mils(&self, p: Pt) -> (i64, i64) {
        let (x, y) = k40::to_mils(p, self.bed[1]);
        (x.clamp(0, (self.bed[0] * k40::MILS_PER_MM) as i64), y.clamp(0, (self.bed[1] * k40::MILS_PER_MM) as i64))
    }
    pub fn pos_mm(&self) -> Pt {
        gburn_core::pt(self.at.0 as f64 / k40::MILS_PER_MM, self.bed[1] - self.at.1 as f64 / k40::MILS_PER_MM)
    }
    fn queue_egv(&mut self, egv: &str) {
        self.queue.extend(k40::packets(egv));
    }
    /// rapid moves through bed points (mm), beam off
    fn rapids(&mut self, pts: &[Pt]) {
        let mut e = Egv::at(self.at.0, self.at.1);
        for p in pts {
            let (x, y) = self.mils(*p);
            e.rapid_to(x, y);
        }
        e.rapid_mode();
        self.at = (e.x, e.y);
        let out = e.out;
        self.queue_egv(&out);
    }
    pub fn jog(&mut self, dx: f64, dy: f64) {
        if self.running() {
            return;
        }
        let p = self.pos_mm();
        self.rapids(&[gburn_core::pt(p.x + dx, p.y + dy)]);
    }
    pub fn frame(&mut self, pts: &[Pt]) {
        if self.running() {
            return;
        }
        self.rapids(pts);
        self.note("Frame (beam off)");
    }
    pub fn home(&mut self) {
        if self.running() {
            return;
        }
        self.queue_egv("IPP\n");
        self.at = (0, 0);
        self.lost = false;
        self.note("Home");
    }
    /// A job's egv (from `k40::job_egv` at `at`) and where it leaves the head.
    pub fn start(&mut self, egv: &str, end: (i64, i64), now: f64) {
        if self.lost {
            self.error = "The head stopped part way through the last move: press Home first, then Start".into();
            self.note(&self.error.clone());
            self.phase = Phase::Failed;
            return;
        }
        self.queue_egv(egv);
        self.total = self.queue.len();
        self.sent = 0;
        self.at = end;
        self.phase = Phase::Sending;
        (self.started, self.job_seconds, self.in_job) = (now, 0.0, true);
        self.error.clear();
        self.note(&format!("Job started: {} packets", self.total));
    }
    /// Pause / resume = `PN` sent ahead of the queue (the board toggles), and the queue held here.
    pub fn pause(&mut self) {
        if matches!(self.phase, Phase::Sending | Phase::Running) {
            let _ = self.usb.write(&k40::frame(b"PN").bytes);
            self.phase = Phase::Paused;
            self.note("Paused");
        }
    }
    pub fn resume(&mut self) {
        if self.phase == Phase::Paused {
            let _ = self.usb.write(&k40::frame(b"PN").bytes);
            self.phase = Phase::Sending;
            self.note("Resumed");
        }
    }
    /// Drop what's waiting, then `I` (the board's interrupt: it stops and empties its buffer).
    pub fn stop(&mut self) {
        let moving = self.busy() || self.running();
        self.queue.clear();
        (self.pending, self.finishing) = (None, None);
        let _ = self.usb.write(&k40::frame(b"I").bytes);
        if moving {
            self.lost = true;
            self.note("Stopped. Press Home before the next job (the head's position is no longer known)");
        }
        if self.running() {
            self.phase = Phase::Idle;
            self.in_job = false;
        }
    }
    fn fail(&mut self, why: String) {
        self.note(&why);
        self.error = why;
        self.queue.clear();
        (self.pending, self.finishing) = (None, None);
        self.lost = true;
        self.phase = Phase::Failed;
        self.in_job = false;
    }

    pub fn tick(&mut self, now: f64) {
        if self.in_job && self.phase != Phase::Paused {
            self.job_seconds = now - self.started;
        }
        // ponytail: blocking USB in the frame, bounded to a few packets (MeerK40t runs this on its own thread)
        for _ in 0..12 {
            if self.phase == Phase::Paused && self.pending.is_none() {
                return;
            }
            if let Some((pk, tries, since)) = self.pending.take() {
                let s = match self.usb.status() {
                    Ok(s) => s,
                    Err(e) => return self.fail(format!("Lost the K40: {e}")),
                };
                match s {
                    k40::STATUS_OK => {
                        self.sent += 1;
                        if pk.wait_finish {
                            self.finishing = Some(now);
                        }
                    }
                    k40::STATUS_ERROR if tries < 8 => {
                        if let Err(e) = self.usb.write(&pk.bytes) {
                            return self.fail(format!("Lost the K40: {e}"));
                        }
                        self.pending = Some((pk, tries + 1, now));
                    }
                    k40::STATUS_ERROR => return self.fail("The K40 board keeps refusing the data (checksum): try another USB cable or port".into()),
                    _ if now - since > 5.0 => return self.fail(format!("No answer from the K40 board (status {s}): is it switched on?")),
                    _ => {
                        self.pending = Some((pk, tries, since));
                        return; // busy: the next frame
                    }
                }
                continue;
            }
            if let Some(since) = self.finishing {
                match self.usb.status() {
                    Ok(s) if s & 0x02 == 0 => self.finishing = None,
                    Ok(_) if now - since > 600.0 => return self.fail("The K40 board never reported the job done".into()),
                    Ok(_) => return,
                    Err(e) => return self.fail(format!("Lost the K40: {e}")),
                }
                continue;
            }
            let Some(pk) = self.queue.pop_front() else { break };
            if let Err(e) = self.usb.write(&pk.bytes) {
                return self.fail(format!("Lost the K40: {e}"));
            }
            self.pending = Some((pk, 0, now));
        }
        if self.phase == Phase::Sending && self.queue.is_empty() && self.pending.is_none() {
            self.phase = if self.finishing.is_some() { Phase::Running } else { Phase::Done };
        }
        if self.phase == Phase::Running && self.finishing.is_none() {
            self.phase = Phase::Done;
        }
        if self.phase == Phase::Done && self.in_job {
            self.in_job = false;
            self.note(&format!("Job done in {:.0} s", self.job_seconds));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// the mock behind a shared handle, so the test can read what was written
    struct Shared(Rc<RefCell<MockK40>>);
    impl Ch341 for Shared {
        fn write(&mut self, b: &[u8]) -> Result<(), String> {
            self.0.borrow_mut().write(b)
        }
        fn status(&mut self) -> Result<u8, String> {
            self.0.borrow_mut().status()
        }
    }

    #[test]
    fn a_job_frames_and_burns_on_a_pretend_board() {
        let m = Rc::new(RefCell::new(MockK40::default()));
        let mut k = K40::new(Box::new(Shared(m.clone())), [300.0, 200.0], 48);
        let text = |m: &MockK40| m.written.iter().map(|p| String::from_utf8_lossy(p).trim_end_matches('F').to_string()).collect::<String>();
        // a frame: rapid moves only, never a D
        k.frame(&[gburn_core::pt(10.0, 190.0), gburn_core::pt(20.0, 190.0), gburn_core::pt(20.0, 180.0)]);
        for i in 0..50 {
            k.tick(i as f64 * 0.02);
        }
        let framed = text(&m.borrow());
        assert!(framed.starts_with("IB") && !framed.contains('D'), "{framed}");
        assert_eq!(k.at, ((20.0 * k40::MILS_PER_MM).round() as i64, (20.0 * k40::MILS_PER_MM).round() as i64));
        // a job: a square from where the frame left the head; one packet refused once is sent again
        let mut d = gburn_core::Document::default();
        d.bed = [300.0, 200.0];
        d.add(gburn_core::geom::make_rect(10.0, 170.0, 20.0, 20.0), 0);
        let plan = gburn_core::plan::plan(&d, gburn_core::plan::Machine::default());
        let (egv, end) = k40::job_egv(&d, &plan, &k40::K40Options { bed: d.bed, home_after: true }, k.at);
        m.borrow_mut().written.clear();
        m.borrow_mut().refuse_next = true;
        k.start(&egv, end, 1.0);
        for i in 0..400 {
            k.tick(1.0 + i as f64 * 0.02);
            if k.phase == Phase::Done {
                break;
            }
        }
        assert_eq!(k.phase, Phase::Done, "{:?}", k.log);
        assert_eq!(k.sent, k.total);
        let sent = text(&m.borrow());
        assert_eq!(sent, egv.replace(['\n', '-'], ""), "every packet arrived once, in order");
        assert!(sent.contains('D') && k.at == (0, 0));
        // Stop: an I goes out and the head counts as lost until Home
        k.jog(5.0, 0.0);
        k.stop();
        assert!(k.lost && String::from_utf8_lossy(m.borrow().written.last().unwrap()).starts_with("IFFF"));
        k.start(&egv, end, 9.0);
        assert_eq!(k.phase, Phase::Failed, "no job until Home");
        k.home();
        assert!(!k.lost && k.at == (0, 0));
    }
}
