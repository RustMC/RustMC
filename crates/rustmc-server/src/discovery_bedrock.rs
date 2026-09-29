//! Bounded Bedrock unconnected discovery only. No RakNet session is established.

pub const VERSION: &str = "1.26.51";
pub const PROTOCOL: u32 = 2193;
pub const MAX_DATAGRAM: usize = 512;
const MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

pub fn pong(input: &[u8], server_guid: u64, port: u16) -> Option<Vec<u8>> {
    if input.len() != 33 || !matches!(input[0], 0x01 | 0x02) || input[9..25] != MAGIC {
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
    (out.len() <= MAX_DATAGRAM).then_some(out)
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
        assert!(String::from_utf8_lossy(&p).contains(";2193;1.26.51;"));
        ping[10] = 0;
        assert!(pong(&ping, 42, 19132).is_none());
        assert!(pong(&vec![0; 513], 42, 19132).is_none());
    }
}
