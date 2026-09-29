//! Bounded Java Edition 26.3 status exchange. Login and play are absent.

pub const PROTOCOL: i32 = 777;
pub const VERSION: &str = "26.3";
pub const MAX_FRAME: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Handshake,
    Status,
    Ping,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Malformed,
    Oversized,
    WrongState,
}

fn varint(input: &[u8]) -> Result<Option<(u32, usize)>, Error> {
    let mut value = 0u32;
    for (i, byte) in input.iter().take(5).enumerate() {
        if i == 4 && byte & 0xf0 != 0 {
            return Err(Error::Malformed);
        }
        value |= u32::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            if i > 0 && *byte == 0 {
                return Err(Error::Malformed);
            }
            return Ok(Some((value, i + 1)));
        }
    }
    if input.len() >= 5 {
        Err(Error::Malformed)
    } else {
        Ok(None)
    }
}

fn put_varint(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn frame(id: u32, body: &[u8]) -> Vec<u8> {
    let mut packet = Vec::new();
    put_varint(id, &mut packet);
    packet.extend_from_slice(body);
    let mut out = Vec::new();
    put_varint(packet.len() as u32, &mut out);
    out.extend(packet);
    out
}

fn status() -> Vec<u8> {
    // Static ASCII is intentionally used; no untrusted content is interpolated.
    let json = format!(
        "{{\"version\":{{\"name\":\"{VERSION}\",\"protocol\":{PROTOCOL}}},\"players\":{{\"max\":0,\"online\":0}},\"description\":{{\"text\":\"RustMC discovery only; login unavailable\"}}}}"
    );
    let mut body = Vec::new();
    put_varint(json.len() as u32, &mut body);
    body.extend_from_slice(json.as_bytes());
    frame(0, &body)
}

pub struct Session {
    state: State,
    input: Vec<u8>,
    bytes: usize,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            state: State::Handshake,
            input: Vec::new(),
            bytes: 0,
        }
    }
}
impl Session {
    pub fn state(&self) -> State {
        self.state
    }
    pub fn receive(&mut self, bytes: &[u8], limit: usize) -> Result<Vec<Vec<u8>>, Error> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .ok_or(Error::Oversized)?;
        if self.bytes > limit || self.input.len().saturating_add(bytes.len()) > MAX_FRAME + 5 {
            return Err(Error::Oversized);
        }
        self.input.extend_from_slice(bytes);
        let mut responses = Vec::new();
        while let Some((size, prefix)) = varint(&self.input)? {
            let size = size as usize;
            if size == 0 {
                return Err(Error::Malformed);
            }
            if size > MAX_FRAME || size > limit {
                return Err(Error::Oversized);
            }
            if self.input.len() < prefix + size {
                break;
            }
            let packet = self.input[prefix..prefix + size].to_vec();
            self.input.drain(..prefix + size);
            let Some((id, id_len)) = varint(&packet)? else {
                return Err(Error::Malformed);
            };
            let body = &packet[id_len..];
            match (self.state, id) {
                (State::Handshake, 0) => {
                    let Some((_version, n)) = varint(body)? else {
                        return Err(Error::Malformed);
                    };
                    let body = &body[n..];
                    let Some((host_len, n)) = varint(body)? else {
                        return Err(Error::Malformed);
                    };
                    let host_len = host_len as usize;
                    if host_len > 255 || body.len() < n + host_len + 2 {
                        return Err(Error::Malformed);
                    }
                    if std::str::from_utf8(&body[n..n + host_len]).is_err() {
                        return Err(Error::Malformed);
                    }
                    let rest = &body[n + host_len + 2..];
                    let Some((next, n)) = varint(rest)? else {
                        return Err(Error::Malformed);
                    };
                    if n != rest.len() || next != 1 {
                        return Err(Error::WrongState);
                    }
                    self.state = State::Status;
                }
                (State::Status, 0) if body.is_empty() => {
                    responses.push(status());
                    self.state = State::Ping;
                }
                (State::Ping, 1) if body.len() == 8 => {
                    responses.push(frame(1, body));
                    self.state = State::Done;
                }
                _ => return Err(Error::WrongState),
            }
        }
        Ok(responses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn handshake(version: u32) -> Vec<u8> {
        let mut body = Vec::new();
        put_varint(version, &mut body);
        body.push(1);
        body.push(b'x');
        body.extend(25565u16.to_be_bytes());
        body.push(1);
        frame(0, &body)
    }
    #[test]
    fn split_status_and_ping() {
        let mut s = Session::default();
        let h = handshake(PROTOCOL as u32);
        for b in h {
            assert!(s.receive(&[b], 4096).unwrap().is_empty());
        }
        assert_eq!(s.state(), State::Status);
        let answer = s.receive(&frame(0, &[]), 4096).unwrap();
        assert!(String::from_utf8_lossy(&answer[0]).contains("777"));
        let nonce = [9u8; 8];
        assert_eq!(
            s.receive(&frame(1, &nonce), 4096).unwrap()[0],
            frame(1, &nonce)
        );
        assert_eq!(s.state(), State::Done);
    }
    #[test]
    fn older_version_gets_current_status() {
        let mut s = Session::default();
        s.receive(&handshake(766), 4096).unwrap();
        assert!(
            String::from_utf8_lossy(&s.receive(&frame(0, &[]), 4096).unwrap()[0]).contains("777")
        );
    }
    #[test]
    fn malformed_and_oversized() {
        assert_eq!(
            Session::default().receive(&[0x80, 0x80, 0x80, 0x80, 0x80], 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            Session::default().receive(&[0x81, 0x20], 4096),
            Err(Error::Oversized)
        );
        assert_eq!(
            Session::default().receive(&frame(0, &[]), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            Session::default().receive(&handshake(777), 3),
            Err(Error::Oversized)
        );
    }
    #[test]
    fn login_is_rejected() {
        let mut h = handshake(777);
        *h.last_mut().unwrap() = 2;
        assert_eq!(Session::default().receive(&h, 4096), Err(Error::WrongState));
    }

    #[test]
    fn invalid_host_text_and_out_of_order_ping_are_rejected() {
        let mut h = handshake(777);
        h[5] = 0xff;
        assert_eq!(Session::default().receive(&h, 4096), Err(Error::Malformed));
        let mut s = Session::default();
        s.receive(&handshake(777), 4096).unwrap();
        assert_eq!(s.receive(&frame(1, &[0; 8]), 4096), Err(Error::WrongState));
    }
}
