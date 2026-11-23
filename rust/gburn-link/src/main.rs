//! GBurn Link: the browser can't send UDP, a Ruida controller only speaks UDP. This helper runs on a computer on
//! the laser's network and relays a WebSocket from the browser to the controller, datagram for datagram
//! (the same idea as LightBurn Bridge, in 200 lines):
//!
//!   ws://127.0.0.1:7451/udp?host=<laser ip>&port=50200&from=40200
//!
//! Every binary WebSocket message from the page goes out as one datagram to host:port from a socket bound to
//! `from`; every datagram that comes back goes to the page as one binary message. Only connections from this
//! computer (127.0.0.1) from gburn.app's pages, and only to private addresses (10/8, 172.16/12, 192.168/16, 169.254/16)
//! on laser ports: a web page can't use it to reach the internet or this computer's own services. Nothing is interpreted here: the app does the Ruida protocol.
//!
//!   ws://127.0.0.1:7451/tcp?host=<controller ip>&port=23
//!
//! relays a byte stream instead (GRBL over the network: FluidNC / grblHAL Telnet), same rules.
//!
//! Run: `gburn-link` (then open gburn.app/app/ in the browser on this computer). `gburn-link --port 7452` for
//! another port, `gburn-link --allow-any` to also relay to public / loopback addresses and any port.

use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tungstenite::handshake::server::{Request, Response};
use tungstenite::{Message, WebSocket};

pub const DEFAULT_PORT: u16 = 7451;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port = args.iter().position(|a| a == "--port").and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);
    let allow_any = args.iter().any(|a| a == "--allow-any");
    match serve(port, allow_any, None) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("GBurn Link couldn't start: {e}");
            std::process::exit(1);
        }
    }
}

/// Listen on 127.0.0.1:`port` until `stop` is set (None = forever). Each connection gets its own thread.
pub fn serve(port: u16, allow_any: bool, stop: Option<Arc<AtomicBool>>) -> std::io::Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
    listener.set_nonblocking(true)?;
    println!("GBurn Link {} listening on ws://127.0.0.1:{port}/  (keep this window open; gburn.app in the browser finds it here)", env!("CARGO_PKG_VERSION"));
    loop {
        if stop.as_ref().is_some_and(|s| s.load(Ordering::Relaxed)) {
            return Ok(());
        }
        match listener.accept() {
            Ok((stream, peer)) => {
                if !peer.ip().is_loopback() {
                    continue; // only this computer's browser
                }
                // macOS / BSD: a socket accepted from a non-blocking listener is non-blocking too, and the handshake
                // then fails with WouldBlock whenever the browser's request hasn't fully arrived. Blocking (with a
                // timeout) for the handshake; relay() goes non-blocking once it is done.
                if stream.set_nonblocking(false).and_then(|_| stream.set_read_timeout(Some(Duration::from_secs(5)))).is_err() {
                    continue;
                }
                std::thread::spawn(move || {
                    if let Err(e) = relay(stream, allow_any) {
                        eprintln!("connection ended: {e}");
                    }
                });
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e),
        }
    }
}

struct Target {
    host: IpAddr,
    port: u16,
    from: u16,
    /// `/tcp`: a byte stream (FluidNC / grblHAL Telnet) instead of datagrams (Ruida)
    tcp: bool,
}

/// `/udp?host=…&port=…&from=…` → where to relay
fn parse_target(uri: &str, allow_any: bool) -> Result<Target, String> {
    let (path, query) = uri.split_once('?').unwrap_or((uri, ""));
    let tcp = match path {
        "/udp" => false,
        "/tcp" => true,
        _ => return Err(format!("unknown path {path} (want /udp or /tcp)")),
    };
    let mut host = None;
    let (mut port, mut from) = (if tcp { 23u16 } else { 50200u16 }, 40200u16);
    for kv in query.split('&') {
        match kv.split_once('=') {
            Some(("host", v)) => host = v.parse::<IpAddr>().ok(),
            Some(("port", v)) => port = v.parse().map_err(|_| "bad port")?,
            Some(("from", v)) => from = v.parse().map_err(|_| "bad from port")?,
            _ => {}
        }
    }
    let host = host.ok_or("host missing or not an IP address")?;
    if allow_any {
        return Ok(Target { host, port, from, tcp });
    }
    // private LAN only: no loopback / 0.0.0.0 (a page mustn't reach services on this computer), no internet
    let private = match host {
        IpAddr::V4(v) => v.is_private() || v.is_link_local(),
        IpAddr::V6(_) => false,
    };
    if !private {
        return Err(format!("{host} is not a private LAN address (start with --allow-any to relay to it)"));
    }
    // only the ports lasers listen on: Ruida 50200 (jog 50207) from 40200 (40207); GRBL Telnet 23, xTool 8780 / 8080
    let ok = if tcp { [23, 8080, 8780].contains(&port) } else { [50200, 50207].contains(&port) && [40200, 40207].contains(&from) };
    if !ok {
        return Err(format!("port {port} is not a laser port (start with --allow-any to relay to it)"));
    }
    Ok(Target { host, port, from, tcp })
}

/// Browsers always send Origin: only gburn.app's pages (and local dev builds) may use the relay. None = not a browser.
fn origin_ok(origin: Option<&str>) -> bool {
    let Some(o) = origin else { return true };
    if ["https://gburn.app", "https://www.gburn.app"].contains(&o) {
        return true;
    }
    ["http://localhost:", "http://127.0.0.1:"].iter().any(|p| o.strip_prefix(p).is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())))
}

/// send on a non-blocking socket: WouldBlock leaves the frame buffered, so flush until it's out
fn send(ws: &mut WebSocket<TcpStream>, m: Message) -> Result<(), String> {
    let mut r = ws.send(m);
    loop {
        match r {
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1));
                r = ws.flush();
            }
            r => return r.map_err(|e| e.to_string()),
        }
    }
}

#[allow(clippy::result_large_err)] // tungstenite's handshake error type is what it is
fn relay(stream: TcpStream, allow_any: bool) -> Result<(), String> {
    let mut target: Option<Result<Target, String>> = None;
    let mut ws: WebSocket<TcpStream> = tungstenite::accept_hdr(stream, |req: &Request, resp: Response| {
        let origin = req.headers().get("origin").map(|v| v.to_str().unwrap_or("?"));
        if !origin_ok(origin) {
            let mut no = tungstenite::handshake::server::ErrorResponse::new(Some("origin not allowed".into()));
            *no.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
            return Err(no);
        }
        target = Some(parse_target(&req.uri().to_string(), allow_any));
        Ok(resp)
    })
    .map_err(|e| e.to_string())?;
    let t = match target {
        Some(Ok(t)) => t,
        Some(Err(e)) => {
            let _ = ws.send(Message::Text(format!("error: {e}").into()));
            let _ = ws.close(None);
            return Err(e);
        }
        None => return Err("no request".into()),
    };
    if t.tcp {
        return relay_tcp(ws, SocketAddr::new(t.host, t.port));
    }
    let udp = UdpSocket::bind(("0.0.0.0", t.from)).map_err(|e| {
        let m = format!("port {}: {e} (is LightBurn or the GBurn desktop app using the laser?)", t.from);
        let _ = ws.send(Message::Text(format!("error: {m}").into()));
        m
    })?;
    udp.set_nonblocking(true).map_err(|e| e.to_string())?;
    let to = SocketAddr::new(t.host, t.port);
    ws.get_mut().set_nonblocking(true).map_err(|e| e.to_string())?;
    let _ = ws.send(Message::Text(format!("ok: relaying to {to} from port {}", t.from).into()));
    println!("relay → {to} from port {}", t.from);
    let mut buf = [0u8; 2048];
    loop {
        let mut idle = true;
        match ws.read() {
            Ok(Message::Binary(b)) => {
                idle = false;
                let _ = udp.send_to(&b, to);
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(Message::Ping(p)) => send(&mut ws, Message::Pong(p))?,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return Ok(()),
            Err(e) => return Err(e.to_string()),
        }
        while let Ok((n, _)) = udp.recv_from(&mut buf) {
            idle = false;
            send(&mut ws, Message::Binary(buf[..n].to_vec().into()))?;
        }
        if idle {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// WebSocket ↔ TCP stream: GRBL over the network (FluidNC / grblHAL Telnet, port 23)
fn relay_tcp(mut ws: WebSocket<TcpStream>, to: SocketAddr) -> Result<(), String> {
    use std::io::{Read, Write};
    let mut tcp = TcpStream::connect_timeout(&to, Duration::from_secs(4)).map_err(|e| {
        let m = format!("{to}: {e} (is the controller on and on this network?)");
        let _ = ws.send(Message::Text(format!("error: {m}").into()));
        m
    })?;
    tcp.set_nonblocking(true).map_err(|e| e.to_string())?;
    let _ = tcp.set_nodelay(true);
    ws.get_mut().set_nonblocking(true).map_err(|e| e.to_string())?;
    let _ = ws.send(Message::Text(format!("ok: relaying to {to} (tcp)").into()));
    println!("relay → {to} (tcp)");
    let mut buf = [0u8; 4096];
    loop {
        let mut idle = true;
        match ws.read() {
            Ok(Message::Binary(b)) => {
                idle = false;
                // non-blocking: write_all would give up part way on WouldBlock and lose bytes
                let mut off = 0;
                while off < b.len() {
                    match tcp.write(&b[off..]) {
                        Ok(0) => return Err("the controller stopped taking data".into()),
                        Ok(n) => off += n,
                        Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::Interrupted => std::thread::sleep(Duration::from_millis(1)),
                        Err(e) => return Err(e.to_string()),
                    }
                }
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(Message::Ping(p)) => send(&mut ws, Message::Pong(p))?,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return Ok(()),
            Err(e) => return Err(e.to_string()),
        }
        match tcp.read(&mut buf) {
            Ok(0) => {
                let _ = ws.send(Message::Text("error: the controller closed the connection".into()));
                return Ok(());
            }
            Ok(n) => {
                idle = false;
                send(&mut ws, Message::Binary(buf[..n].to_vec().into()))?;
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
        if idle {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_parsed_and_public_hosts_refused() {
        let t = parse_target("/udp?host=192.168.1.100&port=50200&from=40200", false).unwrap();
        assert_eq!((t.host.to_string().as_str(), t.port, t.from), ("192.168.1.100", 50200, 40200));
        assert!(parse_target("/udp?host=8.8.8.8", false).is_err());
        assert!(parse_target("/udp?host=8.8.8.8", true).is_ok());
        assert!(parse_target("/other?host=10.0.3.3", false).is_err());
        assert!(parse_target("/udp?host=laser.local", false).is_err());
    }

    /// GRBL over TCP: bytes go to a TCP echo and come back, in order
    #[test]
    fn a_tcp_controller_is_reached_through_the_relay() {
        use std::io::{Read, Write};
        let lis = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp_port = lis.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut c, _)) = lis.accept() {
                let mut buf = [0u8; 64];
                while let Ok(n) = c.read(&mut buf) {
                    if n == 0 || c.write_all(&buf[..n].to_ascii_uppercase()).is_err() {
                        break;
                    }
                }
            }
        });
        let port = 17452;
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        std::thread::spawn(move || serve(port, true, Some(s2)).unwrap()); // loopback stand-in
        std::thread::sleep(Duration::from_millis(100));
        let (mut ws, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}/tcp?host=127.0.0.1&port={tcp_port}")).unwrap();
        let hello = ws.read().unwrap();
        assert!(hello.to_text().unwrap().contains("(tcp)"), "{hello:?}");
        ws.send(Message::Binary(b"$x\n".to_vec().into())).unwrap();
        let mut got = Vec::new();
        while got.len() < 3 {
            got.extend_from_slice(&ws.read().unwrap().into_data());
        }
        assert_eq!(got, b"$X\n");
        assert!(parse_target("/tcp?host=10.0.0.9", false).is_ok_and(|t| t.tcp && t.port == 23));
        stop.store(true, Ordering::Relaxed);
    }

    /// a browser-side client (tungstenite) sends bytes through the relay to a UDP echo and gets them back
    #[test]
    fn bytes_go_through_the_relay_both_ways() {
        let echo = UdpSocket::bind("127.0.0.1:0").unwrap();
        echo.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut buf = [0u8; 100];
            if let Ok((n, from)) = echo.recv_from(&mut buf) {
                let mut r = buf[..n].to_vec();
                r.reverse();
                echo.send_to(&r, from).unwrap();
            }
        });
        let port = 17451;
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        std::thread::spawn(move || serve(port, true, Some(s2)).unwrap()); // loopback stand-in
        std::thread::sleep(Duration::from_millis(100));
        let (mut ws, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}/udp?host=127.0.0.1&port={echo_port}&from=0")).unwrap();
        let hello = ws.read().unwrap();
        assert!(hello.to_text().unwrap().starts_with("ok: relaying"), "{hello:?}");
        ws.send(Message::Binary(vec![1, 2, 3].into())).unwrap();
        let back = ws.read().unwrap();
        assert_eq!(back.into_data().as_ref(), &[3, 2, 1]);
        stop.store(true, Ordering::Relaxed);
    }

    /// by default: foreign pages, loopback, public hosts and other ports are refused
    #[test]
    fn foreign_origins_and_off_limits_targets_are_refused() {
        use tungstenite::client::IntoClientRequest;
        assert!(parse_target("/udp?host=127.0.0.1&port=50200&from=40200", false).is_err());
        assert!(parse_target("/udp?host=::1&port=50200&from=40200", false).is_err());
        assert!(parse_target("/tcp?host=0.0.0.0&port=23", false).is_err());
        assert!(parse_target("/udp?host=10.0.0.5&port=53&from=40200", false).is_err());
        assert!(parse_target("/udp?host=10.0.0.5&port=50200&from=5353", false).is_err());
        assert!(parse_target("/udp?host=10.0.0.5&port=50207&from=40200", false).is_ok());
        assert!(parse_target("/tcp?host=10.0.0.5&port=22", false).is_err());
        assert!(parse_target("/tcp?host=10.0.0.5&port=8780", false).is_ok());
        assert!(parse_target("/tcp?host=127.0.0.1&port=22", true).is_ok());
        for o in ["https://gburn.app", "http://localhost:8000", "http://127.0.0.1:1"] {
            assert!(origin_ok(Some(o)), "{o}");
        }
        for o in ["https://evil.com", "https://gburn.app.evil.com", "http://gburn.app", "http://localhost:", "http://localhost:1@evil.com", "null"] {
            assert!(!origin_ok(Some(o)), "{o}");
        }
        let port = 17453;
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        std::thread::spawn(move || serve(port, false, Some(s2)).unwrap());
        std::thread::sleep(Duration::from_millis(100));
        let mut req = format!("ws://127.0.0.1:{port}/udp?host=192.168.1.100").into_client_request().unwrap();
        req.headers_mut().insert("Origin", "https://evil.example".parse().unwrap());
        assert!(tungstenite::connect(req).is_err(), "a foreign page got a socket");
        let mut req = format!("ws://127.0.0.1:{port}/udp?host=127.0.0.1").into_client_request().unwrap();
        req.headers_mut().insert("Origin", "https://gburn.app".parse().unwrap());
        let (mut ws, _) = tungstenite::connect(req).unwrap();
        let m = ws.read().unwrap();
        assert!(m.to_text().unwrap().starts_with("error:"), "{m:?}");
        let (mut ws2, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}/udp?host=1.1.1.1")).unwrap();
        let m = ws2.read().unwrap();
        assert!(m.to_text().unwrap().starts_with("error:"), "{m:?}");
        stop.store(true, Ordering::Relaxed);
    }
}
