//! Bounded Bedrock unconnected discovery and offline RakNet handshake guard.
//!
//! Validates offline discovery pings, RakNet Open Connection Requests (1 and 2),
//! and version compatibility. Connected frame sets (0x80..=0x8f) and subsequent
//! game sessions are rejected at this offline boundary.

use std::net::SocketAddr;

pub const VERSION: &str = "1.26.51";
pub const PROTOCOL: u32 = 2193;
pub const RAKNET_PROTOCOL_VERSION: u8 = 11;
pub const MIN_MTU: u16 = 576;
pub const MAX_MTU: u16 = 1492;
pub const MAX_DATAGRAM: usize = 1492;
pub const MAX_PONG_DATAGRAM: usize = 512;
pub const MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

/// Encodes a `SocketAddr` into RakNet system address wire representation.
pub fn encode_system_address(addr: &SocketAddr) -> Vec<u8> {
    match addr {
        SocketAddr::V4(v4) => {
            let mut bytes = Vec::with_capacity(7);
            bytes.push(4);
            bytes.extend_from_slice(&v4.ip().octets());
            bytes.extend_from_slice(&v4.port().to_be_bytes());
            bytes
        }
        SocketAddr::V6(v6) => {
            let mut bytes = Vec::with_capacity(29);
            bytes.push(6);
            bytes.extend_from_slice(&23u16.to_be_bytes()); // AF_INET6
            bytes.extend_from_slice(&v6.port().to_be_bytes());
            bytes.extend_from_slice(&v6.flowinfo().to_be_bytes());
            bytes.extend_from_slice(&v6.ip().octets());
            bytes.extend_from_slice(&v6.scope_id().to_be_bytes());
            bytes
        }
    }
}

/// Handle incoming UDP datagrams for Bedrock discovery and offline RakNet handshake with peer address.
pub fn handle_datagram_with_peer(
    input: &[u8],
    server_guid: u64,
    port: u16,
    peer: Option<SocketAddr>,
) -> Option<Vec<u8>> {
    if input.is_empty() || input.len() > MAX_DATAGRAM {
        return None;
    }

    match input[0] {
        0x01 | 0x02 => {
            if input.len() != 33 || input[9..25] != MAGIC {
                return None;
            }
            let motd = format!(
                "MCPE;RustMC discovery only;{PROTOCOL};{VERSION};0;0;{server_guid};Login unavailable;Survival;1;{port};0;"
            );
            let mut out = Vec::with_capacity(35 + motd.len());
            out.push(0x1c);
            out.extend_from_slice(&input[1..9]);
            out.extend_from_slice(&server_guid.to_be_bytes());
            out.extend_from_slice(&MAGIC);
            out.extend_from_slice(&(motd.len() as u16).to_be_bytes());
            out.extend_from_slice(motd.as_bytes());
            (out.len() <= MAX_PONG_DATAGRAM).then_some(out)
        }
        0x05 => {
            // Open Connection Request 1:
            // Validate: 18 <= input.len() <= MAX_DATAGRAM, MAGIC at 1..17, zero-padding at 18..
            if input.len() < 18 || input[1..17] != MAGIC || !input[18..].iter().all(|&b| b == 0) {
                return None;
            }
            if input[17] != RAKNET_PROTOCOL_VERSION {
                // Incompatible Protocol Version (0x19, 26 bytes)
                let mut out = Vec::with_capacity(26);
                out.push(0x19);
                out.push(RAKNET_PROTOCOL_VERSION);
                out.extend_from_slice(&MAGIC);
                out.extend_from_slice(&server_guid.to_be_bytes());
                Some(out)
            } else {
                // Open Connection Reply 1 (0x06, 28 bytes) with MTU clamped to [576, 1492]
                let mtu = (input.len() as u16).clamp(MIN_MTU, MAX_MTU);
                let mut out = Vec::with_capacity(28);
                out.push(0x06);
                out.extend_from_slice(&MAGIC);
                out.extend_from_slice(&server_guid.to_be_bytes());
                out.push(0x00); // server has security = false
                out.extend_from_slice(&mtu.to_be_bytes());
                Some(out)
            }
        }
        0x07 => {
            // Open Connection Request 2:
            // Validate: MAGIC, IPv4 (34 bytes) or IPv6 (56 bytes), MTU in [576, 1492], non-zero client_guid.
            if input.len() != 34 && input.len() != 56 {
                return None;
            }
            if input[1..17] != MAGIC {
                return None;
            }
            let (addr_slice, mtu, client_guid) = if input.len() == 34 {
                if input[17] != 4 {
                    return None;
                }
                let addr = &input[17..24];
                let mtu = u16::from_be_bytes([input[24], input[25]]);
                let guid = u64::from_be_bytes(input[26..34].try_into().unwrap());
                (addr, mtu, guid)
            } else {
                if input[17] != 6 {
                    return None;
                }
                let addr = &input[17..46];
                let mtu = u16::from_be_bytes([input[46], input[47]]);
                let guid = u64::from_be_bytes(input[48..56].try_into().unwrap());
                (addr, mtu, guid)
            };

            if !(MIN_MTU..=MAX_MTU).contains(&mtu) || client_guid == 0 {
                return None;
            }

            // Client address in Reply 2 must be the peer's actual network address.
            let encoded_client_addr = match peer {
                Some(p) => encode_system_address(&p),
                None => addr_slice.to_vec(),
            };

            // Open Connection Reply 2 (0x08)
            let mut out = Vec::with_capacity(1 + 16 + 8 + encoded_client_addr.len() + 2 + 1);
            out.push(0x08);
            out.extend_from_slice(&MAGIC);
            out.extend_from_slice(&server_guid.to_be_bytes());
            out.extend_from_slice(&encoded_client_addr);
            out.extend_from_slice(&mtu.to_be_bytes());
            out.push(0x00); // encryption enabled = false
            Some(out)
        }
        0x80..=0x8f => {
            // Connected frame sets are rejected at this offline boundary.
            None
        }
        _ => None,
    }
}

/// Handle incoming UDP datagrams for Bedrock discovery and offline RakNet handshake.
pub fn handle_datagram(input: &[u8], server_guid: u64, port: u16) -> Option<Vec<u8>> {
    handle_datagram_with_peer(input, server_guid, port, None)
}

/// Backwards-compatible alias for [`handle_datagram`].
pub fn pong(input: &[u8], server_guid: u64, port: u16) -> Option<Vec<u8>> {
    handle_datagram(input, server_guid, port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ping_and_malformed_rejection() {
        let mut ping = vec![1];
        ping.extend([3u8; 8]);
        ping.extend(MAGIC);
        ping.extend([4u8; 8]);
        let p = pong(&ping, 42, 19132).unwrap();
        assert_eq!(p[0], 0x1c);
        assert_eq!(&p[1..9], &[3u8; 8]);
        assert_eq!(&p[9..17], &42u64.to_be_bytes());
        assert_eq!(&p[17..33], &MAGIC);
        assert!(String::from_utf8_lossy(&p).contains(";2193;1.26.51;"));
        assert!(p.len() <= MAX_PONG_DATAGRAM);

        // Ping 0x02 is also accepted
        ping[0] = 2;
        assert!(pong(&ping, 42, 19132).is_some());

        // Corrupted magic
        ping[10] = 0;
        assert!(pong(&ping, 42, 19132).is_none());

        // Oversized ping datagram
        assert!(pong(&vec![0; 513], 42, 19132).is_none());
        assert!(pong(&vec![0; 1493], 42, 19132).is_none());
    }

    #[test]
    fn open_connection_request_1_valid_and_mtu_clamping() {
        let server_guid: u64 = 0x5255_5354_4d43_0001;

        // Minimum valid Open Connection Request 1: len 18 (1 byte id, 16 bytes magic, 1 byte version 11)
        let mut req_min = vec![0x05];
        req_min.extend(MAGIC);
        req_min.push(RAKNET_PROTOCOL_VERSION);
        assert_eq!(req_min.len(), 18);

        let reply_min = pong(&req_min, server_guid, 19132).expect("reply 1 min");
        assert_eq!(reply_min.len(), 28);
        assert_eq!(reply_min[0], 0x06);
        assert_eq!(&reply_min[1..17], &MAGIC);
        assert_eq!(&reply_min[17..25], &server_guid.to_be_bytes());
        assert_eq!(reply_min[25], 0x00);
        // MTU clamped to MIN_MTU (576)
        assert_eq!(&reply_min[26..28], &MIN_MTU.to_be_bytes());

        // Typical MTU request (e.g. 1200 bytes with zero padding)
        let mut req_1200 = vec![0x05];
        req_1200.extend(MAGIC);
        req_1200.push(RAKNET_PROTOCOL_VERSION);
        req_1200.resize(1200, 0x00);

        let reply_1200 = pong(&req_1200, server_guid, 19132).expect("reply 1 1200");
        assert_eq!(reply_1200.len(), 28);
        assert_eq!(reply_1200[0], 0x06);
        assert_eq!(&reply_1200[1..17], &MAGIC);
        assert_eq!(&reply_1200[17..25], &server_guid.to_be_bytes());
        assert_eq!(reply_1200[25], 0x00);
        assert_eq!(&reply_1200[26..28], &1200u16.to_be_bytes());

        // Maximum MTU request (1492 bytes with zero padding)
        let mut req_1492 = vec![0x05];
        req_1492.extend(MAGIC);
        req_1492.push(RAKNET_PROTOCOL_VERSION);
        req_1492.resize(MAX_DATAGRAM, 0x00);

        let reply_1492 = pong(&req_1492, server_guid, 19132).expect("reply 1 1492");
        assert_eq!(reply_1492.len(), 28);
        assert_eq!(&reply_1492[26..28], &MAX_MTU.to_be_bytes());
    }

    #[test]
    fn open_connection_request_1_version_mismatch_returns_incompatible_version() {
        let server_guid: u64 = 0x5255_5354_4d43_0002;

        for incompatible_version in [1, 9, 10, 12, 13, 255] {
            let mut req = vec![0x05];
            req.extend(MAGIC);
            req.push(incompatible_version);
            req.resize(1200, 0x00);

            let reply = pong(&req, server_guid, 19132).expect("incompatible version reply");
            assert_eq!(reply.len(), 26);
            assert_eq!(reply[0], 0x19);
            assert_eq!(reply[1], RAKNET_PROTOCOL_VERSION);
            assert_eq!(&reply[2..18], &MAGIC);
            assert_eq!(&reply[18..26], &server_guid.to_be_bytes());
        }
    }

    #[test]
    fn open_connection_request_2_valid() {
        let server_guid: u64 = 0x5255_5354_4d43_0003;
        let client_guid: u64 = 0x1122_3344_5566_7788;

        // IPv4: 34 bytes (1 + 16 + 7 + 2 + 8)
        let mut req_v4 = vec![0x07];
        req_v4.extend(MAGIC);
        // IPv4 address payload: family 4, IP 127.0.0.1, port 19132
        req_v4.extend([4, 127, 0, 0, 1, 0x4a, 0xbc]);
        req_v4.extend(1400u16.to_be_bytes());
        req_v4.extend(client_guid.to_be_bytes());
        assert_eq!(req_v4.len(), 34);

        let reply_v4 = pong(&req_v4, server_guid, 19132).expect("reply 2 v4");
        assert_eq!(reply_v4.len(), 35);
        assert_eq!(reply_v4[0], 0x08);
        assert_eq!(&reply_v4[1..17], &MAGIC);
        assert_eq!(&reply_v4[17..25], &server_guid.to_be_bytes());
        assert_eq!(&reply_v4[25..32], &[4, 127, 0, 0, 1, 0x4a, 0xbc]);
        assert_eq!(&reply_v4[32..34], &1400u16.to_be_bytes());
        assert_eq!(reply_v4[34], 0x00);

        // IPv6: 56 bytes (1 + 16 + 29 + 2 + 8)
        let mut req_v6 = vec![0x07];
        req_v6.extend(MAGIC);
        // IPv6 address payload: family 6, followed by 28 bytes
        req_v6.push(6);
        req_v6.extend([0xab; 28]);
        req_v6.extend(1200u16.to_be_bytes());
        req_v6.extend(client_guid.to_be_bytes());
        assert_eq!(req_v6.len(), 56);

        let reply_v6 = pong(&req_v6, server_guid, 19132).expect("reply 2 v6");
        assert_eq!(reply_v6.len(), 57);
        assert_eq!(reply_v6[0], 0x08);
        assert_eq!(&reply_v6[1..17], &MAGIC);
        assert_eq!(&reply_v6[17..25], &server_guid.to_be_bytes());
        assert_eq!(&reply_v6[25..54], &req_v6[17..46]);
        assert_eq!(&reply_v6[54..56], &1200u16.to_be_bytes());
        assert_eq!(reply_v6[56], 0x00);

        // With explicit peer SocketAddr: Reply 2 encodes the peer's actual network address
        let peer_v4: SocketAddr = "192.168.1.50:54321".parse().unwrap();
        let reply_peer_v4 = handle_datagram_with_peer(&req_v4, server_guid, 19132, Some(peer_v4))
            .expect("reply 2 peer v4");
        assert_eq!(reply_peer_v4[0], 0x08);
        assert_eq!(
            &reply_peer_v4[25..32],
            &[4, 192, 168, 1, 50, (54321 >> 8) as u8, (54321 & 0xff) as u8],
            "Reply 2 must encode the actual UDP client peer address"
        );
    }

    #[test]
    fn rejections_and_guards() {
        let server_guid: u64 = 0x5255_5354_4d43_0004;
        let client_guid: u64 = 0x9988_7766_5544_3322;

        // Open Connection Request 1 (0x05) malformed checks
        // Truncated (< 18 bytes)
        assert!(pong(&[0x05, 0], server_guid, 19132).is_none());
        let mut req1 = vec![0x05];
        req1.extend(MAGIC);
        // length 17 without protocol byte
        assert_eq!(req1.len(), 17);
        assert!(pong(&req1, server_guid, 19132).is_none());

        // Bad MAGIC in 0x05
        let mut bad_magic_req1 = vec![0x05];
        bad_magic_req1.extend([0u8; 16]);
        bad_magic_req1.push(RAKNET_PROTOCOL_VERSION);
        bad_magic_req1.resize(1200, 0);
        assert!(pong(&bad_magic_req1, server_guid, 19132).is_none());

        // Non-zero padding in 0x05
        let mut non_zero_pad = vec![0x05];
        non_zero_pad.extend(MAGIC);
        non_zero_pad.push(RAKNET_PROTOCOL_VERSION);
        non_zero_pad.resize(1200, 0);
        non_zero_pad[50] = 0xff;
        assert!(pong(&non_zero_pad, server_guid, 19132).is_none());

        // Oversized 0x05 (> MAX_DATAGRAM)
        let mut oversized_req1 = vec![0x05];
        oversized_req1.extend(MAGIC);
        oversized_req1.push(RAKNET_PROTOCOL_VERSION);
        oversized_req1.resize(MAX_DATAGRAM + 1, 0);
        assert!(pong(&oversized_req1, server_guid, 19132).is_none());

        // Open Connection Request 2 (0x07) malformed checks
        // Bad magic
        let mut bad_magic_req2 = vec![0x07];
        bad_magic_req2.extend([0u8; 16]);
        bad_magic_req2.extend([4, 127, 0, 0, 1, 0x4a, 0xbc]);
        bad_magic_req2.extend(1400u16.to_be_bytes());
        bad_magic_req2.extend(client_guid.to_be_bytes());
        assert!(pong(&bad_magic_req2, server_guid, 19132).is_none());

        // Invalid length (neither 34 nor 56)
        let mut wrong_len_req2 = vec![0x07];
        wrong_len_req2.extend(MAGIC);
        wrong_len_req2.extend([4, 127, 0, 0, 1, 0x4a, 0xbc]);
        wrong_len_req2.extend(1400u16.to_be_bytes());
        // Truncated client guid (len 33 instead of 34)
        wrong_len_req2.extend(&client_guid.to_be_bytes()[..7]);
        assert_eq!(wrong_len_req2.len(), 33);
        assert!(pong(&wrong_len_req2, server_guid, 19132).is_none());

        // Length 34 with wrong address family marker (!= 4)
        let mut bad_family_v4 = vec![0x07];
        bad_family_v4.extend(MAGIC);
        bad_family_v4.extend([5, 127, 0, 0, 1, 0x4a, 0xbc]);
        bad_family_v4.extend(1400u16.to_be_bytes());
        bad_family_v4.extend(client_guid.to_be_bytes());
        assert_eq!(bad_family_v4.len(), 34);
        assert!(pong(&bad_family_v4, server_guid, 19132).is_none());

        // Length 56 with wrong address family marker (!= 6)
        let mut bad_family_v6 = vec![0x07];
        bad_family_v6.extend(MAGIC);
        bad_family_v6.push(4);
        bad_family_v6.extend([0xab; 28]);
        bad_family_v6.extend(1200u16.to_be_bytes());
        bad_family_v6.extend(client_guid.to_be_bytes());
        assert_eq!(bad_family_v6.len(), 56);
        assert!(pong(&bad_family_v6, server_guid, 19132).is_none());

        // MTU below minimum (< 576)
        let mut low_mtu = vec![0x07];
        low_mtu.extend(MAGIC);
        low_mtu.extend([4, 127, 0, 0, 1, 0x4a, 0xbc]);
        low_mtu.extend((MIN_MTU - 1).to_be_bytes());
        low_mtu.extend(client_guid.to_be_bytes());
        assert!(pong(&low_mtu, server_guid, 19132).is_none());

        // MTU above maximum (> 1492)
        let mut high_mtu = vec![0x07];
        high_mtu.extend(MAGIC);
        high_mtu.extend([4, 127, 0, 0, 1, 0x4a, 0xbc]);
        high_mtu.extend((MAX_MTU + 1).to_be_bytes());
        high_mtu.extend(client_guid.to_be_bytes());
        assert!(pong(&high_mtu, server_guid, 19132).is_none());

        // Zero client_guid
        let mut zero_guid = vec![0x07];
        zero_guid.extend(MAGIC);
        zero_guid.extend([4, 127, 0, 0, 1, 0x4a, 0xbc]);
        zero_guid.extend(1400u16.to_be_bytes());
        zero_guid.extend(0u64.to_be_bytes());
        assert!(pong(&zero_guid, server_guid, 19132).is_none());

        // Connected frame sets (0x80..=0x8f) return None
        for frame_id in 0x80..=0x8f {
            assert!(pong(&[frame_id, 1, 2, 3], server_guid, 19132).is_none());
        }

        // Other unsupported packets return None
        assert!(pong(&[], server_guid, 19132).is_none());
        assert!(pong(&[0x09, 1, 2, 3], server_guid, 19132).is_none());
        assert!(pong(&[0xfe, 1, 2, 3], server_guid, 19132).is_none());
    }
}
