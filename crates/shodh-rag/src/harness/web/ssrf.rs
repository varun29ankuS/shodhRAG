//! Which IP addresses the agent may connect to.
//!
//! Only globally routable unicast addresses are allowed. Everything that
//! reaches this machine, the local network or special infrastructure is
//! refused: loopback, private (RFC 1918 / ULA), link-local, CGNAT, multicast,
//! broadcast, documentation and benchmarking ranges, and IPv6 forms that
//! embed an IPv4 address (IPv4-mapped, IPv4-compatible, NAT64, 6to4,
//! Teredo), whose embedded address is classified in turn.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Why an address is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BlockedAddress {
    #[error("unspecified address")]
    Unspecified,
    #[error("loopback address (this computer)")]
    Loopback,
    #[error("private network address")]
    Private,
    #[error("link-local address")]
    LinkLocal,
    #[error("shared address space (carrier-grade NAT)")]
    SharedAddressSpace,
    #[error("multicast address")]
    Multicast,
    #[error("broadcast address")]
    Broadcast,
    #[error("reserved or special-purpose address")]
    Reserved,
    #[error("documentation or benchmarking address")]
    Documentation,
}

/// Classify an address; `Ok` means it is publicly routable.
pub fn check_ip(ip: IpAddr) -> Result<(), BlockedAddress> {
    match ip {
        IpAddr::V4(v4) => check_v4(v4),
        IpAddr::V6(v6) => check_v6(v6),
    }
}

fn check_v4(ip: Ipv4Addr) -> Result<(), BlockedAddress> {
    let [a, b, c, _] = ip.octets();
    if ip.is_unspecified() || a == 0 {
        // 0.0.0.0/8: "this network"; connecting to it reaches the host.
        return Err(BlockedAddress::Unspecified);
    }
    if ip.is_loopback() {
        return Err(BlockedAddress::Loopback);
    }
    if ip.is_private() {
        return Err(BlockedAddress::Private);
    }
    if ip.is_link_local() {
        return Err(BlockedAddress::LinkLocal);
    }
    if a == 100 && (64..=127).contains(&b) {
        return Err(BlockedAddress::SharedAddressSpace);
    }
    if ip.is_multicast() {
        return Err(BlockedAddress::Multicast);
    }
    if ip.is_broadcast() {
        return Err(BlockedAddress::Broadcast);
    }
    let documentation = (a == 192 && b == 0 && c == 2)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || (a == 198 && (b == 18 || b == 19));
    if documentation {
        return Err(BlockedAddress::Documentation);
    }
    // 192.0.0.0/24 (IETF protocol assignments), 192.88.99.0/24 (6to4
    // relay anycast), 240.0.0.0/4 (reserved).
    if (a == 192 && b == 0 && c == 0) || (a == 192 && b == 88 && c == 99) || a >= 240 {
        return Err(BlockedAddress::Reserved);
    }
    Ok(())
}

fn check_v6(ip: Ipv6Addr) -> Result<(), BlockedAddress> {
    if ip.is_unspecified() {
        return Err(BlockedAddress::Unspecified);
    }
    if ip.is_loopback() {
        return Err(BlockedAddress::Loopback);
    }
    if let Some(v4) = embedded_v4(ip) {
        return check_v4(v4);
    }
    let segments = ip.segments();
    let first = segments[0];
    if first & 0xff00 == 0xff00 {
        return Err(BlockedAddress::Multicast);
    }
    if first & 0xffc0 == 0xfe80 {
        return Err(BlockedAddress::LinkLocal);
    }
    if first & 0xffc0 == 0xfec0 {
        // Deprecated site-local: still routed internally by some networks.
        return Err(BlockedAddress::Private);
    }
    if first & 0xfe00 == 0xfc00 {
        return Err(BlockedAddress::Private);
    }
    if first == 0x2001 && segments[1] == 0x0db8 {
        return Err(BlockedAddress::Documentation);
    }
    if first == 0x0100 && segments[1..4] == [0, 0, 0] {
        // 100::/64 discard-only.
        return Err(BlockedAddress::Reserved);
    }
    // Only 2000::/3 is allocated global unicast.
    if first & 0xe000 != 0x2000 {
        return Err(BlockedAddress::Reserved);
    }
    // 2001::/23 IETF protocol assignments (Teredo 2001::/32 is handled as
    // an embedded address above).
    if first == 0x2001 && segments[1] < 0x0200 {
        return Err(BlockedAddress::Reserved);
    }
    Ok(())
}

/// The IPv4 address an IPv6 address carries, for the forms that route to
/// it: IPv4-mapped (`::ffff:a.b.c.d`), IPv4-compatible (`::a.b.c.d`),
/// NAT64 (`64:ff9b::/96`, `64:ff9b:1::/48`), 6to4 (`2002::/16`) and Teredo
/// (`2001::/32`, client address stored inverted).
pub fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let tail =
        |hi: u16, lo: u16| Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
    if s[0..5] == [0, 0, 0, 0, 0] && (s[5] == 0xffff || s[5] == 0) {
        // `::` and `::1` are handled by the caller; `::ffff:0:0/96` and the
        // deprecated `::/96` compatible form.
        return Some(tail(s[6], s[7]));
    }
    if s[0] == 0x0064 && s[1] == 0xff9b && (s[2..6] == [0, 0, 0, 0] || s[2] == 0x0001) {
        return Some(tail(s[6], s[7]));
    }
    if s[0] == 0x2002 {
        return Some(tail(s[1], s[2]));
    }
    if s[0] == 0x2001 && s[1] == 0 {
        return Some(tail(!s[6], !s[7]));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> Result<(), BlockedAddress> {
        check_ip(s.parse::<IpAddr>().unwrap())
    }

    #[test]
    fn public_addresses_pass() {
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
            "2a00:1450:4001:80b::200e",
        ] {
            assert_eq!(v4(ip), Ok(()), "{ip}");
        }
    }

    #[test]
    fn local_and_special_ipv4_ranges_are_refused() {
        let cases = [
            ("127.0.0.1", BlockedAddress::Loopback),
            ("127.255.255.254", BlockedAddress::Loopback),
            ("0.0.0.0", BlockedAddress::Unspecified),
            ("0.1.2.3", BlockedAddress::Unspecified),
            ("10.0.0.1", BlockedAddress::Private),
            ("172.16.5.4", BlockedAddress::Private),
            ("172.31.255.255", BlockedAddress::Private),
            ("192.168.1.1", BlockedAddress::Private),
            ("169.254.169.254", BlockedAddress::LinkLocal),
            ("100.64.0.1", BlockedAddress::SharedAddressSpace),
            ("100.127.255.255", BlockedAddress::SharedAddressSpace),
            ("224.0.0.1", BlockedAddress::Multicast),
            ("239.255.255.250", BlockedAddress::Multicast),
            ("255.255.255.255", BlockedAddress::Broadcast),
            ("192.0.2.10", BlockedAddress::Documentation),
            ("198.51.100.7", BlockedAddress::Documentation),
            ("203.0.113.9", BlockedAddress::Documentation),
            ("198.18.0.1", BlockedAddress::Documentation),
            ("240.0.0.1", BlockedAddress::Reserved),
            ("192.0.0.8", BlockedAddress::Reserved),
        ];
        for (ip, expected) in cases {
            assert_eq!(v4(ip), Err(expected), "{ip}");
        }
        assert_eq!(v4("100.63.255.255"), Ok(()));
        assert_eq!(v4("100.128.0.0"), Ok(()));
        assert_eq!(v4("172.32.0.1"), Ok(()));
    }

    #[test]
    fn local_and_special_ipv6_ranges_are_refused() {
        let cases = [
            ("::1", BlockedAddress::Loopback),
            ("::", BlockedAddress::Unspecified),
            ("fe80::1", BlockedAddress::LinkLocal),
            ("fc00::1", BlockedAddress::Private),
            ("fd12:3456:789a::1", BlockedAddress::Private),
            ("fec0::1", BlockedAddress::Private),
            ("ff02::1", BlockedAddress::Multicast),
            ("2001:db8::1", BlockedAddress::Documentation),
            ("100::1", BlockedAddress::Reserved),
            ("4000::1", BlockedAddress::Reserved),
        ];
        for (ip, expected) in cases {
            assert_eq!(v4(ip), Err(expected), "{ip}");
        }
    }

    #[test]
    fn ipv6_forms_embedding_ipv4_are_classified_by_the_ipv4_address() {
        assert_eq!(v4("::ffff:127.0.0.1"), Err(BlockedAddress::Loopback));
        assert_eq!(v4("::ffff:7f00:1"), Err(BlockedAddress::Loopback));
        assert_eq!(v4("::ffff:10.1.2.3"), Err(BlockedAddress::Private));
        assert_eq!(v4("::ffff:169.254.169.254"), Err(BlockedAddress::LinkLocal));
        assert_eq!(v4("::ffff:8.8.8.8"), Ok(()));
        assert_eq!(v4("::192.168.0.1"), Err(BlockedAddress::Private));
        assert_eq!(v4("64:ff9b::7f00:1"), Err(BlockedAddress::Loopback));
        assert_eq!(v4("64:ff9b::808:808"), Ok(()));
        assert_eq!(v4("64:ff9b:1::a00:1"), Err(BlockedAddress::Private));
        assert_eq!(v4("2002:7f00:1::"), Err(BlockedAddress::Loopback));
        assert_eq!(v4("2002:c0a8:101::1"), Err(BlockedAddress::Private));
        assert_eq!(v4("2002:808:808::1"), Ok(()));
        // Teredo stores the client address bit-inverted: !127.0.0.1.
        assert_eq!(
            v4("2001:0:4136:e378:8000:63bf:80ff:fffe"),
            Err(BlockedAddress::Loopback)
        );
    }
}
