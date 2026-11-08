//! The userland forwarder, for the case destination NAT deliberately
//! cannot serve.
//!
//! `curl localhost:8080` is the first thing anyone tries, and it cannot
//! work with firewall rules alone. **Loopback is deliberately excluded
//! from destination NAT**: translating a `127.0.0.1` destination into a
//! routable address is exactly the class of rewrite the kernel refuses,
//! because it turns a packet that could never leave the machine into one
//! that can. So either a userland helper is part of this stage or the
//! success test has to avoid loopback. This is the helper, and it is the
//! same code the IPv6 case will need (RND.md §11.4, §17).
//!
//! It runs in the supervisor, after `clone3` — the single-thread invariant
//! binds only up to the clone, and nothing below is reachable from the
//! child.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::firewall::Publish;

/// A listener forwarding to one container port.
pub struct Forwarder {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    pub published: Publish,
}

impl Forwarder {
    /// Bind the host side and start accepting.
    ///
    /// Binding here rather than relying on NAT is also what makes the port
    /// *reserved*: a second container publishing the same host port fails
    /// immediately with `EADDRINUSE` instead of silently adding a second
    /// DNAT rule that never matches.
    pub fn start(p: Publish) -> std::io::Result<Self> {
        let listener = TcpListener::bind(SocketAddrV4::new(p.host_ip, p.host_port))?;
        listener.set_nonblocking(false)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let target = SocketAddrV4::new(p.container_ip, p.container_port);

        let handle = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if stop_thread.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(client) = conn else { continue };
                // One thread per connection. Crude, and right for a
                // handful of forwarded ports; an event loop here would be
                // a second I/O architecture for no measured gain.
                std::thread::spawn(move || {
                    if let Ok(upstream) = TcpStream::connect(target) {
                        splice_both_ways(client, upstream);
                    }
                });
            }
        });

        Ok(Self {
            stop,
            handle: Some(handle),
            published: p,
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the accept loop by connecting to ourselves once.
        let _ = TcpStream::connect(SocketAddrV4::new(
            if self.published.host_ip.is_unspecified() {
                Ipv4Addr::LOCALHOST
            } else {
                self.published.host_ip
            },
            self.published.host_port,
        ));
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Forwarder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn splice_both_ways(client: TcpStream, upstream: TcpStream) {
    let Ok(client_w) = client.try_clone() else {
        return;
    };
    let Ok(upstream_w) = upstream.try_clone() else {
        return;
    };
    let up = std::thread::spawn(move || copy_until_eof(client, upstream_w));
    copy_until_eof(upstream, client_w);
    let _ = up.join();
}

/// Copy one direction, then shut that direction down.
///
/// `shutdown(Write)` rather than dropping: a protocol where the client
/// signals "done sending" and waits for the rest of the reply - HTTP/1.0
/// without a length, or anything line-based - hangs forever if the half
/// close is not forwarded.
fn copy_until_eof(mut from: TcpStream, mut to: TcpStream) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = to.shutdown(std::net::Shutdown::Write);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener as L;

    /// The property the whole module exists for: a connection to the host
    /// address arrives at the target.
    #[test]
    fn a_connection_is_forwarded_to_the_target() {
        // Stand in for the container with a listener on loopback.
        let target = L::bind("127.0.0.1:0").unwrap();
        let target_port = target.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = target.accept() {
                let mut buf = [0u8; 16];
                let n = s.read(&mut buf).unwrap_or(0);
                let _ = s.write_all(&buf[..n]);
            }
        });

        let free = L::bind("127.0.0.1:0").unwrap();
        let host_port = free.local_addr().unwrap().port();
        drop(free);

        let mut f = Forwarder::start(Publish {
            host_ip: Ipv4Addr::LOCALHOST,
            host_port,
            container_ip: Ipv4Addr::LOCALHOST,
            container_port: target_port,
        })
        .expect("bind the host side");

        let mut c = TcpStream::connect(("127.0.0.1", host_port)).expect("connect");
        c.write_all(b"ping").unwrap();
        let mut back = [0u8; 4];
        c.read_exact(&mut back).expect("read the echo back");
        assert_eq!(&back, b"ping");
        f.stop();
    }

    /// Binding the host port is what reserves it. Without that, a second
    /// container publishing the same port would quietly add a DNAT rule
    /// that never matches.
    #[test]
    fn a_port_already_in_use_is_refused_at_once() {
        let held = L::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        match Forwarder::start(Publish {
            host_ip: Ipv4Addr::LOCALHOST,
            host_port: port,
            container_ip: Ipv4Addr::LOCALHOST,
            container_port: 1,
        }) {
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::AddrInUse),
            Ok(_) => panic!("a port already in use must not bind"),
        }
    }

    #[test]
    fn a_published_port_prints_both_ends() {
        let p = Publish {
            host_ip: Ipv4Addr::UNSPECIFIED,
            host_port: 8080,
            container_ip: Ipv4Addr::new(172, 30, 0, 2),
            container_port: 80,
        };
        assert_eq!(p.to_string(), "0.0.0.0:8080 -> 172.30.0.2:80");
    }
}
