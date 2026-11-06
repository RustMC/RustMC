//! Bedrock Edition packet definitions and wire codec for the discovery target,
//! protocol 2193 (1.26.51).
//!
//! Field order, widths, and packet IDs follow Mojang's protocol schema: release
//! `v1.26.51` of <https://github.com/Mojang/bedrock-protocol-docs>, commit
//! `8667debe0adad27de6c692c88942c489a1193206`, with its wire rules at
//! <https://mojang.github.io/bedrock-protocol-docs/1.26.51/primitives/>. Numeric
//! enum values the schema omits come from Mojang's legacy changelogs and from
//! <https://minecraft.wiki/w/Bedrock_Edition_protocol/Data_Types>, revision
//! 3867656. Mojang's generated enum pages number entries by position, which
//! contradicts its own changelogs for `GameType`, so they are not used for values.
//!
//! Three encodings have no primary source and need a real client to confirm:
//! the contents of the Login `Connection Request`, the NBT form of `StartGame`
//! `Player Property Data`, and the `Player Permissions` field, whose schema
//! combines `int8` with `Compression`. No Bedrock client has exercised this code.

use crate::bedrock::session::BedrockSessionError;
use crate::discovery_bedrock::{PROTOCOL, VERSION};
use std::fmt;

/// Network protocol this module implements, shared with discovery.
pub const BEDROCK_PROTOCOL_VERSION: i32 = PROTOCOL as i32;
/// Game version this module implements, shared with discovery.
pub const BEDROCK_GAME_VERSION: &str = VERSION;

/// Schema `maxItems` for the resource pack lists.
const MAX_PACK_ENTRIES: u32 = 65_535;

pub const ID_LOGIN: u32 = 0x01;
pub const ID_PLAY_STATUS: u32 = 0x02;
pub const ID_RESOURCE_PACKS_INFO: u32 = 0x06;
pub const ID_RESOURCE_PACK_STACK: u32 = 0x07;
pub const ID_RESOURCE_PACK_CLIENT_RESPONSE: u32 = 0x08;
pub const ID_START_GAME: u32 = 0x0B;
pub const ID_NETWORK_SETTINGS: u32 = 0x8F;
pub const ID_REQUEST_NETWORK_SETTINGS: u32 = 0xC1;

// --- LEB128 and ZigZag wire helpers ---

pub fn put_varuint32(mut value: u32, out: &mut Vec<u8>) {
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

pub fn read_varuint32(input: &[u8]) -> Result<(u32, usize), BedrockSessionError> {
    let mut value = 0u32;
    for (i, byte) in input.iter().take(5).enumerate() {
        if i == 4 && byte & 0xf0 != 0 {
            return Err(BedrockSessionError::MalformedPacket(
                "varuint32 overflow".to_string(),
            ));
        }
        value |= u32::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
    }
    Err(BedrockSessionError::MalformedPacket(
        "truncated varuint32".to_string(),
    ))
}

pub fn put_varint32(value: i32, out: &mut Vec<u8>) {
    let zigzag = ((value << 1) ^ (value >> 31)) as u32;
    put_varuint32(zigzag, out);
}

pub fn read_varint32(input: &[u8]) -> Result<(i32, usize), BedrockSessionError> {
    let (zigzag, len) = read_varuint32(input)?;
    let val = ((zigzag >> 1) as i32) ^ (-((zigzag & 1) as i32));
    Ok((val, len))
}

pub fn put_varuint64(mut value: u64, out: &mut Vec<u8>) {
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

pub fn read_varuint64(input: &[u8]) -> Result<(u64, usize), BedrockSessionError> {
    let mut value = 0u64;
    for (i, byte) in input.iter().take(10).enumerate() {
        if i == 9 && byte & 0xfe != 0 {
            return Err(BedrockSessionError::MalformedPacket(
                "varuint64 overflow".to_string(),
            ));
        }
        value |= u64::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
    }
    Err(BedrockSessionError::MalformedPacket(
        "truncated varuint64".to_string(),
    ))
}

pub fn put_varint64(value: i64, out: &mut Vec<u8>) {
    let zigzag = ((value << 1) ^ (value >> 63)) as u64;
    put_varuint64(zigzag, out);
}

pub fn read_varint64(input: &[u8]) -> Result<(i64, usize), BedrockSessionError> {
    let (zigzag, len) = read_varuint64(input)?;
    let val = ((zigzag >> 1) as i64) ^ (-((zigzag & 1) as i64));
    Ok((val, len))
}

pub fn put_bedrock_string(value: &str, out: &mut Vec<u8>) {
    put_varuint32(value.len() as u32, out);
    out.extend_from_slice(value.as_bytes());
}

pub fn read_bedrock_string(input: &[u8]) -> Result<(&str, usize), BedrockSessionError> {
    let (len, prefix) = read_varuint32(input)?;
    let len = len as usize;
    if input.len() < prefix + len {
        return Err(BedrockSessionError::MalformedPacket(
            "truncated string".to_string(),
        ));
    }
    let s = std::str::from_utf8(&input[prefix..prefix + len])
        .map_err(|_| BedrockSessionError::MalformedPacket("invalid utf-8".to_string()))?;
    Ok((s, prefix + len))
}

// --- Base64 URL-safe codec without external dependencies ---

pub fn base64_url_decode(input: &str) -> Result<Vec<u8>, BedrockSessionError> {
    let clean = input.trim_end_matches('=');
    let mut bits = 0u32;
    let mut bit_count = 0;
    let mut out = Vec::new();
    for byte in clean.bytes() {
        let val = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b' ' | b'\r' | b'\n' | b'\t' => continue,
            _ => {
                return Err(BedrockSessionError::MalformedPacket(
                    "invalid base64".to_string(),
                ));
            }
        };
        bits = (bits << 6) | u32::from(val);
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push((bits >> bit_count) as u8);
        }
    }
    Ok(out)
}

pub fn base64_url_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut bits = 0u32;
    let mut bit_count = 0;
    for &byte in input {
        bits = (bits << 8) | u32::from(byte);
        bit_count += 8;
        while bit_count >= 6 {
            bit_count -= 6;
            out.push(TABLE[((bits >> bit_count) & 0x3f) as usize] as char);
        }
    }
    if bit_count > 0 {
        bits <<= 6 - bit_count;
        out.push(TABLE[(bits & 0x3f) as usize] as char);
    }
    out
}

fn malformed(reason: &str) -> BedrockSessionError {
    BedrockSessionError::MalformedPacket(reason.to_string())
}

fn strip_packet_id(bytes: &[u8], expected_id: u32) -> Result<&[u8], BedrockSessionError> {
    let (header, len) = read_varuint32(bytes)?;
    let id = header & 0x3ff;
    if id != expected_id {
        return Err(BedrockSessionError::MalformedPacket(format!(
            "expected packet ID {expected_id}, found {id}"
        )));
    }
    Ok(&bytes[len..])
}

/// Bounds-checked cursor over a packet body.
struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(rest: &'a [u8]) -> Self {
        Self { rest }
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], BedrockSessionError> {
        if self.rest.len() < len {
            return Err(malformed("truncated packet body"));
        }
        let (head, tail) = self.rest.split_at(len);
        self.rest = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], BedrockSessionError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    fn bool(&mut self) -> Result<bool, BedrockSessionError> {
        Ok(self.bytes(1)?[0] != 0)
    }

    fn f32(&mut self) -> Result<f32, BedrockSessionError> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    fn varuint32(&mut self) -> Result<u32, BedrockSessionError> {
        let (value, len) = read_varuint32(self.rest)?;
        self.rest = &self.rest[len..];
        Ok(value)
    }

    fn varint32(&mut self) -> Result<i32, BedrockSessionError> {
        let (value, len) = read_varint32(self.rest)?;
        self.rest = &self.rest[len..];
        Ok(value)
    }

    fn varuint64(&mut self) -> Result<u64, BedrockSessionError> {
        let (value, len) = read_varuint64(self.rest)?;
        self.rest = &self.rest[len..];
        Ok(value)
    }

    fn varint64(&mut self) -> Result<i64, BedrockSessionError> {
        let (value, len) = read_varint64(self.rest)?;
        self.rest = &self.rest[len..];
        Ok(value)
    }

    fn string(&mut self) -> Result<&'a str, BedrockSessionError> {
        let (value, len) = read_bedrock_string(self.rest)?;
        self.rest = &self.rest[len..];
        Ok(value)
    }

    /// Reads a collection size that RustMC only ever emits as empty.
    fn expect_empty(&mut self, field: &str, size: u32) -> Result<(), BedrockSessionError> {
        if size != 0 {
            return Err(BedrockSessionError::MalformedPacket(format!(
                "unsupported non-empty {field}"
            )));
        }
        Ok(())
    }

    fn finish(self) -> Result<(), BedrockSessionError> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(malformed("trailing bytes after packet body"))
        }
    }
}

// --- Modeled Packets ---

/// Client-to-server request for network settings (Packet 0xC1 / 193).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestNetworkSettings {
    pub protocol_version: i32,
}

impl RequestNetworkSettings {
    pub fn new(protocol_version: i32) -> Self {
        Self { protocol_version }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varuint32(ID_REQUEST_NETWORK_SETTINGS, &mut out);
        out.extend_from_slice(&self.protocol_version.to_be_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let mut reader = Reader::new(strip_packet_id(bytes, ID_REQUEST_NETWORK_SETTINGS)?);
        let protocol_version = i32::from_be_bytes(reader.array()?);
        reader.finish()?;
        Ok(Self { protocol_version })
    }
}

impl From<i32> for RequestNetworkSettings {
    fn from(protocol_version: i32) -> Self {
        Self::new(protocol_version)
    }
}

impl From<&RequestNetworkSettings> for RequestNetworkSettings {
    fn from(req: &RequestNetworkSettings) -> Self {
        req.clone()
    }
}

/// Server-to-client network configuration parameters (Packet 0x8F / 143).
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkSettings {
    /// Smallest payload to compress; `0` disables compression.
    pub compression_threshold: u16,
    pub compression_algorithm: u16,
    pub client_throttle_enabled: bool,
    pub client_throttle_threshold: u8,
    pub client_throttle_scalar: f32,
}

impl NetworkSettings {
    /// Settings with client throttling disabled.
    pub fn new(compression_threshold: u16, compression_algorithm: u16) -> Self {
        Self {
            compression_threshold,
            compression_algorithm,
            client_throttle_enabled: false,
            client_throttle_threshold: 0,
            client_throttle_scalar: 0.0,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varuint32(ID_NETWORK_SETTINGS, &mut out);
        out.extend_from_slice(&self.compression_threshold.to_le_bytes());
        out.extend_from_slice(&self.compression_algorithm.to_le_bytes());
        out.push(u8::from(self.client_throttle_enabled));
        out.push(self.client_throttle_threshold);
        out.extend_from_slice(&self.client_throttle_scalar.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let mut reader = Reader::new(strip_packet_id(bytes, ID_NETWORK_SETTINGS)?);
        let settings = Self {
            compression_threshold: u16::from_le_bytes(reader.array()?),
            compression_algorithm: u16::from_le_bytes(reader.array()?),
            client_throttle_enabled: reader.bool()?,
            client_throttle_threshold: reader.array::<1>()?[0],
            client_throttle_scalar: reader.f32()?,
        };
        reader.finish()?;
        Ok(settings)
    }
}

/// Player identity claimed by a Bedrock Login JWT chain.
///
/// Token signatures are not verified, so these claims are unauthenticated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BedrockIdentity {
    pub display_name: String,
    pub xuid: String,
    pub identity_public_key: String,
}

/// Client-to-server login request packet (Packet 0x01).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    pub protocol_version: i32,
    pub payload: Vec<u8>,
}

impl Login {
    pub fn new(protocol_version: i32, payload: Vec<u8>) -> Self {
        Self {
            protocol_version,
            payload,
        }
    }

    /// Construct a valid mock Login packet with signed-like JWT identity tokens.
    pub fn create(
        protocol_version: i32,
        display_name: &str,
        xuid: &str,
        identity_public_key: &str,
    ) -> Self {
        let claim_json = serde_json::json!({
            "iss": "Mojang",
            "identityPublicKey": identity_public_key,
            "extraData": {
                "displayName": display_name,
                "XUID": xuid,
                "identity": uuid::Uuid::new_v4().to_string(),
            }
        });
        let header_json = serde_json::json!({
            "alg": "ES384",
            "x5u": identity_public_key,
        });
        let token = format!(
            "{}.{}.{}",
            base64_url_encode(header_json.to_string().as_bytes()),
            base64_url_encode(claim_json.to_string().as_bytes()),
            base64_url_encode(b"attested_signature")
        );
        let chain_json = serde_json::json!({
            "chain": [token]
        });
        let chain_bytes = chain_json.to_string().into_bytes();

        let mut payload = Vec::new();
        payload.extend_from_slice(&(chain_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(&chain_bytes);

        let client_data_token = format!(
            "{}.{}.{}",
            base64_url_encode(b"{}"),
            base64_url_encode(b"{}"),
            base64_url_encode(b"sig")
        );
        payload.extend_from_slice(&(client_data_token.len() as u32).to_le_bytes());
        payload.extend_from_slice(client_data_token.as_bytes());

        Self {
            protocol_version,
            payload,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varuint32(ID_LOGIN, &mut out);
        out.extend_from_slice(&self.protocol_version.to_be_bytes());
        put_varuint32(self.payload.len() as u32, &mut out);
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let mut reader = Reader::new(strip_packet_id(bytes, ID_LOGIN)?);
        let protocol_version = i32::from_be_bytes(reader.array()?);
        let len = reader.varuint32()? as usize;
        let payload = reader.bytes(len)?.to_vec();
        reader.finish()?;
        Ok(Self {
            protocol_version,
            payload,
        })
    }

    /// Extract the claimed public key, display name, and XUID from the JWT chain.
    ///
    /// The `Connection Request` is opaque in Mojang's schema. This reads the shape
    /// [`Login::create`] writes: a little-endian `u32` length, then a JSON object
    /// whose `chain` array holds JWTs. A real client's payload has not been checked
    /// against it. The result comes from the first token carrying `extraData`,
    /// because earlier certificates can hold only an `identityPublicKey`.
    pub fn extract_identity(&self) -> Result<BedrockIdentity, BedrockSessionError> {
        let mut reader = Reader::new(&self.payload);
        let len = u32::from_le_bytes(reader.array()?) as usize;
        let document: serde_json::Value = serde_json::from_slice(reader.bytes(len)?)
            .map_err(|e| BedrockSessionError::MalformedPacket(e.to_string()))?;

        document
            .get("chain")
            .and_then(|chain| chain.as_array())
            .into_iter()
            .flatten()
            .filter_map(|token| token.as_str())
            .find_map(parse_jwt_identity)
            .ok_or_else(|| malformed("no identity token in login payload"))
    }
}

impl From<&Login> for Login {
    fn from(login: &Login) -> Self {
        login.clone()
    }
}

/// Reads the identity claims of one JWT, or `None` if it holds no player identity.
fn parse_jwt_identity(jwt: &str) -> Option<BedrockIdentity> {
    let mut parts = jwt.split('.');
    let (Some(_header), Some(claims), Some(_signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let claims = base64_url_decode(claims).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&claims).ok()?;

    let extra_data = claims.get("extraData")?;
    let display_name = extra_data.get("displayName")?.as_str()?;
    let identity_public_key = claims.get("identityPublicKey")?.as_str()?;
    if display_name.is_empty() || identity_public_key.is_empty() {
        return None;
    }
    let xuid = extra_data
        .get("XUID")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    Some(BedrockIdentity {
        display_name: display_name.to_string(),
        xuid: xuid.to_string(),
        identity_public_key: identity_public_key.to_string(),
    })
}

/// Status of the login or spawn progression (Packet 0x02).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum PlayStatus {
    LoginSuccess = 0,
    FailedClient = 1,
    FailedServer = 2,
    PlayerSpawn = 3,
    FailedInvalidTenant = 4,
    FailedVanillaEdu = 5,
    FailedEduVanilla = 6,
    FailedServerFull = 7,
    FailedEditorVanilla = 8,
    FailedVanillaEditor = 9,
}

impl PlayStatus {
    pub fn from_i32(val: i32) -> Option<Self> {
        match val {
            0 => Some(Self::LoginSuccess),
            1 => Some(Self::FailedClient),
            2 => Some(Self::FailedServer),
            3 => Some(Self::PlayerSpawn),
            4 => Some(Self::FailedInvalidTenant),
            5 => Some(Self::FailedVanillaEdu),
            6 => Some(Self::FailedEduVanilla),
            7 => Some(Self::FailedServerFull),
            8 => Some(Self::FailedEditorVanilla),
            9 => Some(Self::FailedVanillaEditor),
            _ => None,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varuint32(ID_PLAY_STATUS, &mut out);
        out.extend_from_slice(&(*self as i32).to_be_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let mut reader = Reader::new(strip_packet_id(bytes, ID_PLAY_STATUS)?);
        let code = i32::from_be_bytes(reader.array()?);
        reader.finish()?;
        Self::from_i32(code).ok_or_else(|| {
            BedrockSessionError::MalformedPacket(format!("unknown PlayStatus code: {code}"))
        })
    }
}

impl fmt::Display for PlayStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Server resource packs metadata (Packet 0x06).
///
/// RustMC offers no packs, so the world template is empty and the list has no
/// entries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourcePacksInfo {
    pub must_accept: bool,
    pub has_addon_packs: bool,
    pub has_scripts: bool,
    pub force_disable_vibrant_visuals: bool,
}

impl ResourcePacksInfo {
    pub fn new(
        must_accept: bool,
        has_addon_packs: bool,
        has_scripts: bool,
        force_disable_vibrant_visuals: bool,
    ) -> Self {
        Self {
            must_accept,
            has_addon_packs,
            has_scripts,
            force_disable_vibrant_visuals,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varuint32(ID_RESOURCE_PACKS_INFO, &mut out);
        out.push(u8::from(self.must_accept));
        out.push(u8::from(self.has_addon_packs));
        out.push(u8::from(self.has_scripts));
        out.push(u8::from(self.force_disable_vibrant_visuals));
        out.extend_from_slice(&[0; 16]); // World template UUID
        put_bedrock_string("", &mut out); // World template version
        put_varuint32(0, &mut out); // Resource packs
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let mut reader = Reader::new(strip_packet_id(bytes, ID_RESOURCE_PACKS_INFO)?);
        let info = Self {
            must_accept: reader.bool()?,
            has_addon_packs: reader.bool()?,
            has_scripts: reader.bool()?,
            force_disable_vibrant_visuals: reader.bool()?,
        };
        reader.bytes(16)?;
        reader.string()?;
        let packs = reader.varuint32()?;
        reader.expect_empty("resource pack list", packs)?;
        reader.finish()?;
        Ok(info)
    }
}

/// Response status code sent by client in ResourcePackClientResponse.
///
/// Mojang names these `Cancel`, `Downloading`, `DownloadingFinished`, and
/// `ResourcePackStackFinished`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ResponseStatus {
    Refused = 1,
    SendPacks = 2,
    HaveAllPacks = 3,
    Completed = 4,
}

impl ResponseStatus {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            1 => Some(Self::Refused),
            2 => Some(Self::SendPacks),
            3 => Some(Self::HaveAllPacks),
            4 => Some(Self::Completed),
            _ => None,
        }
    }
}

/// Client reply regarding resource packs (Packet 0x08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourcePackClientResponse {
    pub status: ResponseStatus,
}

impl ResourcePackClientResponse {
    pub fn new(status: ResponseStatus) -> Self {
        Self { status }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varuint32(ID_RESOURCE_PACK_CLIENT_RESPONSE, &mut out);
        out.push(self.status as u8);
        if self.status == ResponseStatus::SendPacks {
            put_varuint32(0, &mut out); // Downloading Packs
        }
        out
    }

    /// Reads the response tag only. Bytes after it are not validated: RustMC offers
    /// no packs, so a `Downloading` list has nothing to select from.
    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let mut reader = Reader::new(strip_packet_id(bytes, ID_RESOURCE_PACK_CLIENT_RESPONSE)?);
        let tag = reader.array::<1>()?[0];
        let status = ResponseStatus::from_u8(tag).ok_or_else(|| {
            BedrockSessionError::MalformedPacket(format!("invalid ResponseStatus byte: {tag}"))
        })?;
        Ok(Self { status })
    }
}

impl From<ResponseStatus> for ResourcePackClientResponse {
    fn from(status: ResponseStatus) -> Self {
        Self::new(status)
    }
}

impl From<&ResponseStatus> for ResourcePackClientResponse {
    fn from(status: &ResponseStatus) -> Self {
        Self::new(*status)
    }
}

impl From<&ResourcePackClientResponse> for ResourcePackClientResponse {
    fn from(resp: &ResourcePackClientResponse) -> Self {
        resp.clone()
    }
}

/// Entry descriptor within a resource pack stack.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PackInstance {
    pub pack_id: String,
    pub version: String,
    pub sub_pack_name: String,
}

impl PackInstance {
    pub fn new(
        pack_id: impl Into<String>,
        version: impl Into<String>,
        sub_pack_name: impl Into<String>,
    ) -> Self {
        Self {
            pack_id: pack_id.into(),
            version: version.into(),
            sub_pack_name: sub_pack_name.into(),
        }
    }
}

/// Server enforcement of resource pack application order (Packet 0x07).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourcePackStack {
    pub must_accept: bool,
    pub packs: Vec<PackInstance>,
}

impl ResourcePackStack {
    pub fn new(must_accept: bool) -> Self {
        Self {
            must_accept,
            packs: Vec::new(),
        }
    }

    pub fn with_packs(must_accept: bool, packs: Vec<PackInstance>) -> Self {
        Self { must_accept, packs }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varuint32(ID_RESOURCE_PACK_STACK, &mut out);
        out.push(u8::from(self.must_accept));

        put_varuint32(self.packs.len() as u32, &mut out);
        for pack in &self.packs {
            put_bedrock_string(&pack.pack_id, &mut out);
            put_bedrock_string(&pack.version, &mut out);
            put_bedrock_string(&pack.sub_pack_name, &mut out);
        }

        put_bedrock_string(BEDROCK_GAME_VERSION, &mut out);
        out.extend_from_slice(&0u32.to_le_bytes()); // Experiment toggles
        out.push(0); // Experiments ever toggled
        out.push(0); // Include editor packs
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let mut reader = Reader::new(strip_packet_id(bytes, ID_RESOURCE_PACK_STACK)?);
        let must_accept = reader.bool()?;

        let count = reader.varuint32()?;
        if count > MAX_PACK_ENTRIES {
            return Err(malformed("too many resource pack stack entries"));
        }
        let mut packs = Vec::new();
        for _ in 0..count {
            let pack_id = reader.string()?;
            let version = reader.string()?;
            let sub_pack = reader.string()?;
            packs.push(PackInstance::new(pack_id, version, sub_pack));
        }

        reader.string()?; // Base game version
        let toggles = u32::from_le_bytes(reader.array()?);
        reader.expect_empty("experiment list", toggles)?;
        reader.bool()?; // Experiments ever toggled
        reader.bool()?; // Include editor packs
        reader.finish()?;
        Ok(Self { must_accept, packs })
    }
}

/// `LevelSettings.Server Chunk Tick Range`: the default four-chunk simulation
/// distance offered by the game's world settings.
const SERVER_CHUNK_TICK_RANGE: i32 = 4;

/// An empty root compound for `Player Property Data`: tag type, empty name, end
/// tag. Mojang's schema leaves this field untyped, so the encoding is unconfirmed.
const EMPTY_NBT_COMPOUND: [u8; 3] = [0x0a, 0x00, 0x00];

/// Server-to-client world initialization and player entity spawn packet (Packet 0x0B / 11).
///
/// The fields here are the ones RustMC models. [`StartGame::encode`] writes the
/// complete protocol 2193 layout, filling everything else with the neutral value
/// noted beside it. [`StartGame::decode`] walks the same layout and rejects
/// non-empty collections that RustMC never emits.
#[derive(Debug, Clone, PartialEq)]
pub struct StartGame {
    pub entity_id: i64,
    pub runtime_id: u64,
    pub player_gamemode: i32,
    pub player_pos: (f32, f32, f32),
    pub pitch_yaw: (f32, f32),
    pub seed: i64,
    pub dimension: i32,
    pub generator: i32,
    pub world_gamemode: i32,
    pub difficulty: i32,
    pub spawn_pos: (i32, i32, i32),
    pub level_id: String,
    pub world_name: String,
    pub level_current_time: u64,
    pub enchantment_seed: i32,
    pub multiplayer_correlation_id: String,
    pub server_version: String,
}

impl StartGame {
    pub fn pitch(&self) -> f32 {
        self.pitch_yaw.0
    }

    pub fn yaw(&self) -> f32 {
        self.pitch_yaw.1
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varuint32(ID_START_GAME, &mut out);
        put_varint64(self.entity_id, &mut out);
        put_varuint64(self.runtime_id, &mut out);
        put_varint32(self.player_gamemode, &mut out);
        out.extend_from_slice(&self.player_pos.0.to_le_bytes());
        out.extend_from_slice(&self.player_pos.1.to_le_bytes());
        out.extend_from_slice(&self.player_pos.2.to_le_bytes());
        out.extend_from_slice(&self.pitch_yaw.0.to_le_bytes());
        out.extend_from_slice(&self.pitch_yaw.1.to_le_bytes());
        self.encode_level_settings(&mut out);
        put_bedrock_string(&self.level_id, &mut out);
        put_bedrock_string(&self.world_name, &mut out);
        put_bedrock_string("", &mut out); // Template content identity
        out.push(0); // Is trial
        put_varint32(0, &mut out); // Movement: rewind history size
        out.push(0); // Movement: server authoritative block breaking
        out.extend_from_slice(&self.level_current_time.to_le_bytes());
        put_varint32(self.enchantment_seed, &mut out);
        put_varuint32(0, &mut out); // Block properties
        put_bedrock_string(&self.multiplayer_correlation_id, &mut out);
        out.push(0); // Enable item stack net manager
        put_bedrock_string(&self.server_version, &mut out);
        out.extend_from_slice(&EMPTY_NBT_COMPOUND); // Player property data
        out.extend_from_slice(&0u64.to_le_bytes()); // Block type registry checksum
        out.extend_from_slice(&[0; 16]); // World template ID
        out.push(0); // Server enabled client-side generation
        out.push(0); // Block network IDs are hashes
        out.push(0); // Server auth sound enabled
        out.push(0); // Server configuration join info: absent
        for _ in 0..4 {
            put_bedrock_string("", &mut out); // Telemetry: server, scenario, world, owner
        }
        out
    }

    fn encode_level_settings(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.seed.to_le_bytes());
        out.extend_from_slice(&0i16.to_le_bytes()); // Spawn biome type: Default
        put_bedrock_string("", out); // User defined biome name
        put_varint32(self.dimension, out);
        put_varint32(self.generator, out);
        put_varint32(self.world_gamemode, out);
        out.push(0); // Is hardcore
        put_varint32(self.difficulty, out);
        put_varint32(self.spawn_pos.0, out);
        put_varint32(self.spawn_pos.1, out);
        put_varint32(self.spawn_pos.2, out);
        out.push(1); // Achievements disabled: RustMC tracks none
        put_varint32(0, out); // Editor world type: NonEditor
        out.push(0); // Is created in editor
        out.push(0); // Is exported from editor
        put_varint32(0, out); // Day cycle stop time
        put_varuint32(0, out); // Education edition offer: None
        out.push(0); // Education features enabled
        put_bedrock_string("", out); // Education product ID
        out.extend_from_slice(&0f32.to_le_bytes()); // Rain level
        out.extend_from_slice(&0f32.to_le_bytes()); // Lightning level
        out.push(0); // Has confirmed platform locked content
        out.push(1); // Multiplayer game intent
        out.push(0); // LAN broadcast intent
        put_varint32(0, out); // Xbox Live broadcast setting: NoMultiPlay
        put_varint32(0, out); // Platform broadcast setting: NoMultiPlay
        out.push(0); // Commands enabled
        out.push(0); // Texture packs required
        put_varuint32(0, out); // Game rules
        out.extend_from_slice(&0u32.to_le_bytes()); // Experiment toggles
        out.push(0); // Experiments ever toggled
        out.push(0); // Has bonus chest enabled
        out.push(0); // Start with map enabled
        // Player permissions: Visitor. The schema marks this int8 field `Compression`,
        // which Mojang defines only for 32- and 64-bit integers. Zero is the one value
        // that is the same byte whether the field is raw or a zigzag varint.
        out.push(0);
        out.extend_from_slice(&SERVER_CHUNK_TICK_RANGE.to_le_bytes());
        out.push(0); // Has locked behavior pack
        out.push(0); // Has locked resource pack
        out.push(0); // Is from locked template
        out.push(0); // Use MSA gamertags only
        out.push(0); // Is from world template
        out.push(0); // Is world template option locked
        out.push(0); // Only spawn V1 villagers
        out.push(0); // Persona disabled
        out.push(0); // Custom skins disabled
        out.push(0); // Emote chat muted
        put_bedrock_string(BEDROCK_GAME_VERSION, out); // Base game version
        out.extend_from_slice(&0i32.to_le_bytes()); // Limited world width
        out.extend_from_slice(&0i32.to_le_bytes()); // Limited world depth
        out.push(0); // Nether type
        put_bedrock_string("", out); // Edu shared resource: button name
        put_bedrock_string("", out); // Edu shared resource: link URI
        out.push(0); // Override force experimental gameplay: absent
        out.push(0); // Chat restriction level: None
        out.push(0); // Disable player interactions
        put_varint32(0, out); // Server editor connection policy: MatchWorldType
        out.push(0); // Allow anonymous block drops in editor worlds
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let mut r = Reader::new(strip_packet_id(bytes, ID_START_GAME)?);
        let entity_id = r.varint64()?;
        let runtime_id = r.varuint64()?;
        let player_gamemode = r.varint32()?;
        let player_pos = (r.f32()?, r.f32()?, r.f32()?);
        let pitch_yaw = (r.f32()?, r.f32()?);

        // LevelSettings.
        let seed = i64::from_le_bytes(r.array()?);
        r.bytes(2)?; // Spawn biome type
        r.string()?; // User defined biome name
        let dimension = r.varint32()?;
        let generator = r.varint32()?;
        let world_gamemode = r.varint32()?;
        r.bool()?; // Is hardcore
        let difficulty = r.varint32()?;
        let spawn_pos = (r.varint32()?, r.varint32()?, r.varint32()?);
        r.bool()?; // Achievements disabled
        r.varint32()?; // Editor world type
        r.bytes(2)?; // Is created in, exported from editor
        r.varint32()?; // Day cycle stop time
        r.varuint32()?; // Education edition offer
        r.bool()?; // Education features enabled
        r.string()?; // Education product ID
        r.bytes(8)?; // Rain and lightning levels
        r.bytes(3)?; // Locked content, multiplayer intent, LAN intent
        r.varint32()?; // Xbox Live broadcast setting
        r.varint32()?; // Platform broadcast setting
        r.bytes(2)?; // Commands enabled, texture packs required
        let rules = r.varuint32()?;
        r.expect_empty("game rule list", rules)?;
        let toggles = u32::from_le_bytes(r.array()?);
        r.expect_empty("experiment list", toggles)?;
        r.bytes(3)?; // Experiments ever toggled, bonus chest, start with map
        r.bytes(1)?; // Player permissions
        r.bytes(4)?; // Server chunk tick range
        r.bytes(10)?; // Locked packs through emote chat muted
        r.string()?; // Base game version
        r.bytes(8)?; // Limited world width and depth
        r.bool()?; // Nether type
        r.string()?; // Edu shared resource: button name
        r.string()?; // Edu shared resource: link URI
        if r.bool()? {
            r.bool()?; // Override force experimental gameplay
        }
        r.bytes(2)?; // Chat restriction level, disable player interactions
        r.varint32()?; // Server editor connection policy
        r.bool()?; // Allow anonymous block drops in editor worlds

        let level_id = r.string()?.to_string();
        let world_name = r.string()?.to_string();
        r.string()?; // Template content identity
        r.bool()?; // Is trial
        r.varint32()?; // Movement: rewind history size
        r.bool()?; // Movement: server authoritative block breaking
        let level_current_time = u64::from_le_bytes(r.array()?);
        let enchantment_seed = r.varint32()?;
        let block_properties = r.varuint32()?;
        r.expect_empty("block property list", block_properties)?;
        let multiplayer_correlation_id = r.string()?.to_string();
        r.bool()?; // Enable item stack net manager
        let server_version = r.string()?.to_string();
        if r.bytes(EMPTY_NBT_COMPOUND.len())? != EMPTY_NBT_COMPOUND {
            return Err(malformed("unsupported player property data"));
        }
        r.bytes(8)?; // Block type registry checksum
        r.bytes(16)?; // World template ID
        r.bytes(3)?; // Client-side generation, hashed IDs, auth sound
        if r.bool()? {
            return Err(malformed("unsupported server configuration join info"));
        }
        for _ in 0..4 {
            r.string()?; // Telemetry: server, scenario, world, owner
        }
        r.finish()?;

        Ok(Self {
            entity_id,
            runtime_id,
            player_gamemode,
            player_pos,
            pitch_yaw,
            seed,
            dimension,
            generator,
            world_gamemode,
            difficulty,
            spawn_pos,
            level_id,
            world_name,
            level_current_time,
            enchantment_seed,
            multiplayer_correlation_id,
            server_version,
        })
    }
}

/// Unified enumeration of all supported Bedrock packets in protocol 2193.
#[derive(Debug, Clone, PartialEq)]
pub enum BedrockPacket {
    RequestNetworkSettings(RequestNetworkSettings),
    NetworkSettings(NetworkSettings),
    Login(Login),
    PlayStatus(PlayStatus),
    ResourcePacksInfo(ResourcePacksInfo),
    ResourcePackClientResponse(ResourcePackClientResponse),
    ResourcePackStack(ResourcePackStack),
    StartGame(StartGame),
}

impl BedrockPacket {
    pub fn packet_id(&self) -> u32 {
        match self {
            Self::RequestNetworkSettings(_) => ID_REQUEST_NETWORK_SETTINGS,
            Self::NetworkSettings(_) => ID_NETWORK_SETTINGS,
            Self::Login(_) => ID_LOGIN,
            Self::PlayStatus(_) => ID_PLAY_STATUS,
            Self::ResourcePacksInfo(_) => ID_RESOURCE_PACKS_INFO,
            Self::ResourcePackClientResponse(_) => ID_RESOURCE_PACK_CLIENT_RESPONSE,
            Self::ResourcePackStack(_) => ID_RESOURCE_PACK_STACK,
            Self::StartGame(_) => ID_START_GAME,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::RequestNetworkSettings(p) => p.encode(),
            Self::NetworkSettings(p) => p.encode(),
            Self::Login(p) => p.encode(),
            Self::PlayStatus(p) => p.encode(),
            Self::ResourcePacksInfo(p) => p.encode(),
            Self::ResourcePackClientResponse(p) => p.encode(),
            Self::ResourcePackStack(p) => p.encode(),
            Self::StartGame(p) => p.encode(),
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let (header, _) = read_varuint32(bytes)?;
        let id = header & 0x3ff;
        match id {
            ID_REQUEST_NETWORK_SETTINGS => Ok(Self::RequestNetworkSettings(
                RequestNetworkSettings::decode(bytes)?,
            )),
            ID_NETWORK_SETTINGS => Ok(Self::NetworkSettings(NetworkSettings::decode(bytes)?)),
            ID_LOGIN => Ok(Self::Login(Login::decode(bytes)?)),
            ID_PLAY_STATUS => Ok(Self::PlayStatus(PlayStatus::decode(bytes)?)),
            ID_RESOURCE_PACKS_INFO => {
                Ok(Self::ResourcePacksInfo(ResourcePacksInfo::decode(bytes)?))
            }
            ID_RESOURCE_PACK_CLIENT_RESPONSE => Ok(Self::ResourcePackClientResponse(
                ResourcePackClientResponse::decode(bytes)?,
            )),
            ID_RESOURCE_PACK_STACK => {
                Ok(Self::ResourcePackStack(ResourcePackStack::decode(bytes)?))
            }
            ID_START_GAME => Ok(Self::StartGame(StartGame::decode(bytes)?)),
            _ => Err(BedrockSessionError::MalformedPacket(format!(
                "unsupported Bedrock packet ID: {id}"
            ))),
        }
    }

    pub fn encode_framed(&self) -> Vec<u8> {
        let body = self.encode();
        let mut framed = Vec::with_capacity(5 + body.len());
        put_varuint32(body.len() as u32, &mut framed);
        framed.extend_from_slice(&body);
        framed
    }

    pub fn decode_framed(bytes: &[u8]) -> Result<(Self, usize), BedrockSessionError> {
        let (len, prefix) = read_varuint32(bytes)?;
        let len = len as usize;
        if bytes.len() < prefix + len {
            return Err(BedrockSessionError::MalformedPacket(
                "truncated framed packet".to_string(),
            ));
        }
        let pkt = Self::decode(&bytes[prefix..prefix + len])?;
        Ok((pkt, prefix + len))
    }

    pub fn as_network_settings(&self) -> Option<&NetworkSettings> {
        match self {
            Self::NetworkSettings(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_play_status(&self) -> Option<&PlayStatus> {
        match self {
            Self::PlayStatus(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_resource_packs_info(&self) -> Option<&ResourcePacksInfo> {
        match self {
            Self::ResourcePacksInfo(info) => Some(info),
            _ => None,
        }
    }

    pub fn as_resource_pack_stack(&self) -> Option<&ResourcePackStack> {
        match self {
            Self::ResourcePackStack(stack) => Some(stack),
            _ => None,
        }
    }

    pub fn as_start_game(&self) -> Option<&StartGame> {
        match self {
            Self::StartGame(game) => Some(game),
            _ => None,
        }
    }
}
