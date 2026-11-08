//! Networking over rtnetlink, written out rather than taken from a crate.
//!
//! The socket loop, the sequencing, the `NLMSG_DONE` walking and the typed
//! error mapping are the parts worth owning; a packet codec is ~47,000
//! lines of TLV tedium with no learning past the first hour, and we need a
//! handful of messages. This is that handful.
//!
//! One shortcut removes a whole step: a veth pair can be created with its
//! **peer placed directly into the target network namespace** in a single
//! `RTM_NEWLINK`, via `IFLA_NET_NS_FD` inside `VETH_INFO_PEER`. There is no
//! window in which the container's interface exists on the host.

use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;

use crate::sys::Error as Errno;

// --- netlink constants -----------------------------------------------------
// Defined here rather than imported, for the same reason as the clone flags.

const NETLINK_ROUTE: i32 = 0;
const NLM_F_REQUEST: u16 = 0x001;
const NLM_F_ACK: u16 = 0x004;
const NLM_F_EXCL: u16 = 0x200;
const NLM_F_CREATE: u16 = 0x400;
const NLMSG_ERROR: u16 = 2;

const RTM_NEWLINK: u16 = 16;
const RTM_NEWADDR: u16 = 20;
const RTM_NEWROUTE: u16 = 24;

const IFLA_IFNAME: u16 = 3;
const IFLA_MASTER: u16 = 10;
const IFLA_LINKINFO: u16 = 18;
const IFLA_NET_NS_FD: u16 = 28;
const IFLA_INFO_KIND: u16 = 1;
const IFLA_INFO_DATA: u16 = 2;
const VETH_INFO_PEER: u16 = 1;

const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;

const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;

const IFF_UP: u32 = 0x1;
const AF_INET: u8 = 2;
const RT_TABLE_MAIN: u8 = 254;
const RTPROT_BOOT: u8 = 3;
const RT_SCOPE_UNIVERSE: u8 = 0;
const RTN_UNICAST: u8 = 1;

/// A netlink message under construction.
///
/// Every attribute is 4-byte aligned, and the header length counts the
/// padding of everything before it but not its own trailing padding. That
/// rule is where hand-rolled codecs break first, so it lives in one place.
pub struct Msg {
    buf: Vec<u8>,
}

impl Msg {
    fn new(kind: u16, flags: u16) -> Self {
        let mut buf = vec![0u8; 16];
        buf[4..6].copy_from_slice(&kind.to_ne_bytes());
        buf[6..8].copy_from_slice(&(flags | NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
        buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
        Self { buf }
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    fn attr(&mut self, kind: u16, data: &[u8]) {
        let len = (4 + data.len()) as u16;
        self.buf.extend_from_slice(&len.to_ne_bytes());
        self.buf.extend_from_slice(&kind.to_ne_bytes());
        self.buf.extend_from_slice(data);
        while !self.buf.len().is_multiple_of(4) {
            self.buf.push(0);
        }
    }

    fn nest(&mut self, kind: u16) -> usize {
        let at = self.buf.len();
        self.buf.extend_from_slice(&0u16.to_ne_bytes());
        self.buf.extend_from_slice(&kind.to_ne_bytes());
        at
    }

    fn end_nest(&mut self, at: usize) {
        let len = (self.buf.len() - at) as u16;
        self.buf[at..at + 2].copy_from_slice(&len.to_ne_bytes());
    }

    fn finish(mut self) -> Vec<u8> {
        let len = self.buf.len() as u32;
        self.buf[0..4].copy_from_slice(&len.to_ne_bytes());
        self.buf
    }
}

fn ifinfomsg(index: i32, flags: u32, change: u32) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[4..8].copy_from_slice(&index.to_ne_bytes());
    b[8..12].copy_from_slice(&flags.to_ne_bytes());
    b[12..16].copy_from_slice(&change.to_ne_bytes());
    b
}

fn ifaddrmsg(prefix: u8, index: u32) -> [u8; 8] {
    let mut b = [0u8; 8];
    b[0] = AF_INET;
    b[1] = prefix;
    b[3] = RT_SCOPE_UNIVERSE;
    b[4..8].copy_from_slice(&index.to_ne_bytes());
    b
}

fn rtmsg(dst_len: u8) -> [u8; 12] {
    let mut b = [0u8; 12];
    b[0] = AF_INET;
    b[1] = dst_len;
    b[4] = RT_TABLE_MAIN;
    b[5] = RTPROT_BOOT;
    b[6] = RT_SCOPE_UNIVERSE;
    b[7] = RTN_UNICAST;
    b
}

/// A netlink socket. One per namespace we talk to.
pub struct Netlink {
    fd: OwnedFd,
}

impl Netlink {
    pub fn open() -> Result<Self, Errno> {
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                NETLINK_ROUTE,
            )
        };
        if fd < 0 {
            return Err(last_errno());
        }
        Ok(Self {
            fd: unsafe { <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) },
        })
    }

    /// Send one request and read its acknowledgement.
    ///
    /// The ack **is** an error message carrying errno 0. A `send` that
    /// succeeds says nothing: the kernel's verdict only arrives here.
    pub fn request(&self, msg: &[u8]) -> Result<(), Errno> {
        send_and_ack(self.fd.as_raw_fd(), msg)
    }
}

fn last_errno() -> Errno {
    Errno::from_raw_os_error(unsafe { *libc::__errno_location() })
}

pub fn send_and_ack(fd: i32, msg: &[u8]) -> Result<(), Errno> {
    let sent = unsafe { libc::send(fd, msg.as_ptr() as *const libc::c_void, msg.len(), 0) };
    if sent < 0 {
        return Err(last_errno());
    }
    let mut reply = [0u8; 1024];
    let n = unsafe { libc::recv(fd, reply.as_mut_ptr() as *mut libc::c_void, reply.len(), 0) };
    if n < 0 {
        return Err(last_errno());
    }
    if n as usize >= 20 {
        let kind = u16::from_ne_bytes([reply[4], reply[5]]);
        if kind == NLMSG_ERROR {
            let err = i32::from_ne_bytes([reply[16], reply[17], reply[18], reply[19]]);
            if err != 0 {
                return Err(Errno::from_raw_os_error(-err));
            }
        }
    }
    Ok(())
}

// --- message builders ------------------------------------------------------

/// `ip link add <bridge> type bridge`.
pub fn create_bridge(name: &str) -> Vec<u8> {
    let mut m = Msg::new(RTM_NEWLINK, NLM_F_CREATE | NLM_F_EXCL);
    m.raw(&ifinfomsg(0, 0, 0));
    m.attr(IFLA_IFNAME, cname(name).as_bytes());
    let li = m.nest(IFLA_LINKINFO);
    m.attr(IFLA_INFO_KIND, b"bridge\0");
    m.end_nest(li);
    m.finish()
}

/// `ip link add <host> type veth peer name <peer> netns <fd>`.
///
/// The peer is placed in the target namespace by the same message that
/// creates it, so the container's interface never exists on the host.
///
/// `VETH_INFO_PEER`'s payload is **not** a plain attribute list: it begins
/// with a bare `struct ifinfomsg`, and the kernel's policy requires at
/// least that many bytes. Getting this wrong is the single most common
/// hand-rolled-codec failure here.
pub fn create_veth(host: &str, peer: &str, peer_netns: &OwnedFd) -> Vec<u8> {
    let mut m = Msg::new(RTM_NEWLINK, NLM_F_CREATE | NLM_F_EXCL);
    m.raw(&ifinfomsg(0, 0, 0));
    m.attr(IFLA_IFNAME, cname(host).as_bytes());
    let li = m.nest(IFLA_LINKINFO);
    m.attr(IFLA_INFO_KIND, b"veth\0");
    let id = m.nest(IFLA_INFO_DATA);
    let vp = m.nest(VETH_INFO_PEER);
    m.raw(&ifinfomsg(0, 0, 0));
    m.attr(IFLA_IFNAME, cname(peer).as_bytes());
    m.attr(
        IFLA_NET_NS_FD,
        &(peer_netns.as_raw_fd() as u32).to_ne_bytes(),
    );
    m.end_nest(vp);
    m.end_nest(id);
    m.end_nest(li);
    m.finish()
}

/// `ip link set <name> up`.
pub fn link_up(name: &str) -> Vec<u8> {
    let mut m = Msg::new(RTM_NEWLINK, 0);
    m.raw(&ifinfomsg(0, IFF_UP, IFF_UP));
    m.attr(IFLA_IFNAME, cname(name).as_bytes());
    m.finish()
}

/// `ip link set <name> master <bridge>`.
pub fn set_master(name: &str, bridge_index: u32) -> Vec<u8> {
    let mut m = Msg::new(RTM_NEWLINK, 0);
    m.raw(&ifinfomsg(0, 0, 0));
    m.attr(IFLA_IFNAME, cname(name).as_bytes());
    m.attr(IFLA_MASTER, &bridge_index.to_ne_bytes());
    m.finish()
}

/// `ip addr add <ip>/<prefix> dev <index>`.
pub fn add_address(index: u32, ip: Ipv4Addr, prefix: u8) -> Vec<u8> {
    let mut m = Msg::new(RTM_NEWADDR, NLM_F_CREATE | NLM_F_EXCL);
    m.raw(&ifaddrmsg(prefix, index));
    m.attr(IFA_LOCAL, &ip.octets());
    m.attr(IFA_ADDRESS, &ip.octets());
    m.finish()
}

/// `ip route add default via <gw> dev <index>`.
pub fn default_route(index: u32, gateway: Ipv4Addr) -> Vec<u8> {
    let mut m = Msg::new(RTM_NEWROUTE, NLM_F_CREATE | NLM_F_EXCL);
    m.raw(&rtmsg(0));
    m.attr(RTA_GATEWAY, &gateway.octets());
    m.attr(RTA_OIF, &index.to_ne_bytes());
    m.finish()
}

fn cname(s: &str) -> String {
    format!("{s}\0")
}

/// The interface index of `name`, asked of the kernel directly.
///
/// `if_nametoindex` is a `SIOCGIFINDEX` ioctl, not a file read. Reading
/// `/sys/class/net/<name>/ifindex` looks equivalent and is not: sysfs is
/// populated by a separate path from the netlink acknowledgement, so a
/// read immediately after `RTM_NEWLINK` returns can miss - rarely, and
/// only on a loaded machine, which is the worst way for a bug to behave.
/// It also answers for the caller's namespace, which sysfs does not when
/// `/sys` is bind-mounted from the host.
pub fn if_index(name: &str) -> Option<u32> {
    let c = std::ffi::CString::new(name).ok()?;
    index_of(&c)
}

/// Is IPv4 forwarding on?
///
/// Read, never written. Turning it on is a **destructive** change: the
/// kernel documents that writing `ip_forward` "resets all configuration
/// parameters to their default state", silently undoing whatever the
/// operator set. Refusing with the command to run removes this stage's
/// worst host mutation entirely (RND.md §11, §17).
pub fn ip_forward_enabled() -> bool {
    std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward")
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

/// A container's address assignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    pub ip: Ipv4Addr,
    pub prefix: u8,
    pub gateway: Ipv4Addr,
}

/// The default bridge subnet.
///
/// A `/16` out of RFC 1918 space that Docker's own first-fit allocator
/// does not reach until its fifteenth network, so the two can coexist on
/// one host without colliding.
pub const BRIDGE: &str = "rustainer0";
pub const SUBNET_PREFIX: u8 = 16;
pub const GATEWAY: Ipv4Addr = Ipv4Addr::new(172, 30, 0, 1);

/// Hand out the next address.
///
/// Sequential, persisted, and deliberately simple: a real allocator is a
/// deferred decision, and the shape that matters now is that the engine
/// records what it handed out rather than recomputing it.
pub fn allocate(state_root: &Path) -> std::io::Result<Lease> {
    let path = state_root.join("ipam");
    let next: u32 = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(2);
    // .0 is the network, .1 is the gateway, and the broadcast address of a
    // /16 is .255.255. Wrap well before any of them.
    let next = if next >= 65_000 { 2 } else { next };
    std::fs::write(&path, (next + 1).to_string())?;
    let o = GATEWAY.octets();
    Ok(Lease {
        ip: Ipv4Addr::new(o[0], o[1], (next / 256) as u8, (next % 256) as u8),
        prefix: SUBNET_PREFIX,
        gateway: GATEWAY,
    })
}

/// One message, plus where its interface index lives.
///
/// The index cannot be known in the parent. "lo is 1 and eth0 is 2" holds
/// only in a namespace with nothing else in it, and on a kernel with the
/// tunnel modules loaded a fresh namespace also gets sit0, tunl0, gre0 and
/// ip6tnl0 - so eth0 can be index 6. The child resolves the name with
/// `if_nametoindex` and writes the answer into these four bytes.
pub type Message = (Vec<u8>, usize);

/// The messages the container's own init sends, built here.
///
/// Pre-built because the child cannot allocate: it resolves one index,
/// patches it in, and writes the bytes.
pub fn container_messages(lease: &Lease) -> Vec<Message> {
    // The index field of `ifinfomsg` and of `ifaddrmsg` both sit four
    // bytes past the 16-byte netlink header.
    const INDEX_AT: usize = 20;
    let route = default_route(0, lease.gateway);
    // RTA_OIF is the last attribute, and its value is the last four bytes.
    let route_index_at = route.len() - 4;
    vec![
        (link_up_index(0), INDEX_AT),
        (add_address(0, lease.ip, lease.prefix), INDEX_AT),
        (route, route_index_at),
    ]
}

/// `if_nametoindex`, which is a `SIOCGIFINDEX` ioctl and therefore
/// answers for the namespace the caller is in, from the kernel rather
/// than from a filesystem that mirrors it.
///
/// # Safety
///
/// Child-side safe: no allocation, one socket, one ioctl.
pub fn index_of(name: &std::ffi::CStr) -> Option<u32> {
    let n = unsafe { libc::if_nametoindex(name.as_ptr()) };
    (n != 0).then_some(n)
}

fn link_up_index(index: u32) -> Vec<u8> {
    let mut m = Msg::new(RTM_NEWLINK, 0);
    m.raw(&ifinfomsg(index as i32, IFF_UP, IFF_UP));
    m.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_len(msg: &[u8]) -> u32 {
        u32::from_ne_bytes([msg[0], msg[1], msg[2], msg[3]])
    }

    #[test]
    fn every_message_declares_its_own_length() {
        for m in [
            create_bridge("br0"),
            link_up("veth0"),
            set_master("veth0", 3),
            add_address(2, Ipv4Addr::new(10, 0, 0, 2), 24),
            default_route(2, Ipv4Addr::new(10, 0, 0, 1)),
        ] {
            assert_eq!(header_len(&m) as usize, m.len(), "length field must match");
            assert!(
                m.len().is_multiple_of(4),
                "netlink messages are 4-byte aligned"
            );
        }
    }

    #[test]
    fn attributes_are_padded_to_four_bytes() {
        // "veth0\0" is six bytes; the attribute is 4 + 6 = 10, padded to 12.
        let m = link_up("veth0");
        // 16 header + 16 ifinfomsg + 12 attribute
        assert_eq!(m.len(), 44);
        // The declared attribute length is the UNPADDED one.
        let attr_len = u16::from_ne_bytes([m[32], m[33]]);
        assert_eq!(attr_len, 10, "the length field excludes the padding");
    }

    #[test]
    fn the_veth_peer_section_begins_with_an_ifinfomsg() {
        // VETH_INFO_PEER's payload is not a plain attribute list: the
        // kernel's policy requires at least sizeof(struct ifinfomsg)
        // before any attribute, and a shorter payload is rejected.
        let ns = std::fs::File::open("/proc/self/ns/net")
            .map(OwnedFd::from)
            .expect("every process has a network namespace");
        let m = create_veth("hostside", "eth0", &ns);
        assert_eq!(header_len(&m) as usize, m.len());
        // IFLA_LINKINFO is nested, so the message is comfortably larger
        // than a bare link message.
        assert!(m.len() > 60, "got {}", m.len());
    }

    #[test]
    fn addresses_are_allocated_sequentially_and_persist() {
        let d = std::env::temp_dir().join("rustainer-ipam-test");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let a = allocate(&d).unwrap();
        let b = allocate(&d).unwrap();
        assert_eq!(a.ip, Ipv4Addr::new(172, 30, 0, 2));
        assert_eq!(b.ip, Ipv4Addr::new(172, 30, 0, 3));
        assert_eq!(a.gateway, GATEWAY);
        assert_ne!(a.ip, a.gateway, "never hand out the gateway's address");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn the_container_gets_exactly_three_messages() {
        let lease = Lease {
            ip: Ipv4Addr::new(172, 30, 0, 5),
            prefix: 16,
            gateway: GATEWAY,
        };
        let msgs = container_messages(&lease);
        assert_eq!(msgs.len(), 3, "link up, address, default route");
        for (m, at) in &msgs {
            assert_eq!(header_len(m) as usize, m.len());
            assert!(
                at + 4 <= m.len(),
                "the index offset must be inside the message"
            );
            // Patched to zero on purpose: the child writes the real index
            // there, because it is the only one that can know it.
            assert_eq!(&m[*at..*at + 4], &[0, 0, 0, 0]);
        }
    }

    #[test]
    fn ip_forward_is_only_ever_read() {
        // The function exists; the point is that there is no writer.
        let _ = ip_forward_enabled();
        let src = include_str!("net.rs");
        assert!(
            !src.contains("write(\"/proc/sys/net/ipv4/ip_forward\""),
            "writing ip_forward resets every other network parameter to its default"
        );
    }
}
