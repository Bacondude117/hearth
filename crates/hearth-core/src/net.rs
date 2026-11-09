//! Port helpers.

use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, UdpSocket};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

/// True when nothing on this machine is bound to `port` for `protocol`.
pub fn port_is_free(port: u16, protocol: Protocol) -> bool {
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
    match protocol {
        Protocol::Tcp => TcpListener::bind(addr).is_ok(),
        Protocol::Udp => UdpSocket::bind(addr).is_ok(),
    }
}

/// First port at or after `start` that is free on this machine and not in `reserved`.
pub fn next_free_port(start: u16, protocol: Protocol, reserved: &[u16]) -> Option<u16> {
    (start..=u16::MAX).find(|p| !reserved.contains(p) && port_is_free(*p, protocol))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_bound_tcp_port() {
        let l = TcpListener::bind("0.0.0.0:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(!port_is_free(port, Protocol::Tcp));
        drop(l);
        assert!(port_is_free(port, Protocol::Tcp));
    }

    #[test]
    fn next_free_skips_reserved_and_bound() {
        let l = UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let got = next_free_port(port, Protocol::Udp, &[port + 1]).unwrap();
        assert_ne!(got, port);
        assert_ne!(got, port + 1);
        assert!(got > port);
    }
}
