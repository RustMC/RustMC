//! Bedrock Edition packet definitions and wire codec for protocol version 766 (v1.21.50).

use crate::bedrock::session::BedrockSessionError;
use std::fmt;

pub const BEDROCK_PROTOCOL_VERSION: i32 = 766;
pub const BEDROCK_GAME_VERSION: &str = "1.21.50";

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

fn strip_packet_id(bytes: &[u8], expected_id: u32) -> &[u8] {
    match read_varuint32(bytes) {
        Ok((id, len)) if id & 0x3ff == expected_id => &bytes[len..],
        _ => bytes,
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
        let rest = strip_packet_id(bytes, ID_REQUEST_NETWORK_SETTINGS);
        if rest.len() < 4 {
            return Err(BedrockSessionError::MalformedPacket(
                "RequestNetworkSettings too short".to_string(),
            ));
        }
        let protocol_version = i32::from_be_bytes(rest[0..4].try_into().unwrap());
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSettings {
    pub compression_threshold: u16,
    pub compression_algorithm: u16,
}

impl NetworkSettings {
    pub fn new(compression_threshold: u16, compression_algorithm: u16) -> Self {
        Self {
            compression_threshold,
            compression_algorithm,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varuint32(ID_NETWORK_SETTINGS, &mut out);
        out.extend_from_slice(&self.compression_threshold.to_le_bytes());
        out.extend_from_slice(&self.compression_algorithm.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let rest = strip_packet_id(bytes, ID_NETWORK_SETTINGS);
        if rest.len() < 4 {
            return Err(BedrockSessionError::MalformedPacket(
                "NetworkSettings too short".to_string(),
            ));
        }
        let compression_threshold = u16::from_le_bytes(rest[0..2].try_into().unwrap());
        let compression_algorithm = u16::from_le_bytes(rest[2..4].try_into().unwrap());
        Ok(Self {
            compression_threshold,
            compression_algorithm,
        })
    }
}

/// Extracted player authentication identity from Bedrock Login JWT chain.
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
        let rest = strip_packet_id(bytes, ID_LOGIN);
        if rest.len() < 4 {
            return Err(BedrockSessionError::MalformedPacket(
                "Login packet too short".to_string(),
            ));
        }
        let protocol_version = i32::from_be_bytes(rest[0..4].try_into().unwrap());
        let payload_bytes = &rest[4..];
        let payload = if let Ok((len, pfx)) = read_varuint32(payload_bytes) {
            let len = len as usize;
            if payload_bytes.len() >= pfx + len {
                payload_bytes[pfx..pfx + len].to_vec()
            } else {
                payload_bytes.to_vec()
            }
        } else {
            payload_bytes.to_vec()
        };
        Ok(Self {
            protocol_version,
            payload,
        })
    }

    /// Extract public key, displayName, and XUID from the payload JWT chain or JSON structure.
    pub fn extract_identity(&self) -> Result<BedrockIdentity, BedrockSessionError> {
        if self.payload.is_empty() {
            return Err(BedrockSessionError::MalformedPacket(
                "empty login payload".to_string(),
            ));
        }

        // Check if there is a 4-byte LE length prefix for the chain JSON string
        let payload_str = if self.payload.len() >= 4 {
            let len = u32::from_le_bytes(self.payload[0..4].try_into().unwrap()) as usize;
            if self.payload.len() >= 4 + len && len > 0 {
                std::str::from_utf8(&self.payload[4..4 + len]).ok()
            } else {
                std::str::from_utf8(&self.payload).ok()
            }
        } else {
            std::str::from_utf8(&self.payload).ok()
        };

        let payload_str = payload_str.ok_or_else(|| {
            BedrockSessionError::MalformedPacket("payload is not utf-8".to_string())
        })?;

        // 1. Parse as JSON structure
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(payload_str) {
            if let Some(chain) = val.get("chain").and_then(|c| c.as_array()) {
                for token_val in chain {
                    let maybe_ident = token_val.as_str().and_then(|s| parse_jwt_identity(s).ok());
                    if let Some(identity) = maybe_ident {
                        return Ok(identity);
                    }
                }
            }
            if let Some(display_name) = val.get("displayName").and_then(|v| v.as_str()) {
                let xuid = val
                    .get("xuid")
                    .or_else(|| val.get("XUID"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let pubkey = val
                    .get("identityPublicKey")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                return Ok(BedrockIdentity {
                    display_name: display_name.to_string(),
                    xuid: xuid.to_string(),
                    identity_public_key: pubkey.to_string(),
                });
            }
        }

        // 2. Direct single JWT string
        if let Ok(identity) = parse_jwt_identity(payload_str) {
            return Ok(identity);
        }

        Err(BedrockSessionError::MalformedPacket(
            "no valid identity tokens found in login payload".to_string(),
        ))
    }
}

impl From<&Login> for Login {
    fn from(login: &Login) -> Self {
        login.clone()
    }
}

fn parse_jwt_identity(jwt: &str) -> Result<BedrockIdentity, BedrockSessionError> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() < 2 {
        return Err(BedrockSessionError::MalformedPacket(
            "invalid jwt token parts".to_string(),
        ));
    }
    let claim_bytes = base64_url_decode(parts[1])?;
    let claim_val: serde_json::Value = serde_json::from_slice(&claim_bytes)
        .map_err(|e| BedrockSessionError::MalformedPacket(e.to_string()))?;

    let identity_public_key = claim_val
        .get("identityPublicKey")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let extra_data = claim_val.get("extraData");
    let display_name = extra_data
        .and_then(|e| e.get("displayName"))
        .or_else(|| claim_val.get("displayName"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let xuid = extra_data
        .and_then(|e| e.get("XUID").or_else(|| e.get("xuid")))
        .or_else(|| claim_val.get("XUID").or_else(|| claim_val.get("xuid")))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if display_name.is_empty() && xuid.is_empty() && identity_public_key.is_empty() {
        return Err(BedrockSessionError::MalformedPacket(
            "empty jwt claims".to_string(),
        ));
    }

    Ok(BedrockIdentity {
        display_name,
        xuid,
        identity_public_key,
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
        let rest = strip_packet_id(bytes, ID_PLAY_STATUS);
        if rest.len() < 4 {
            return Err(BedrockSessionError::MalformedPacket(
                "PlayStatus packet too short".to_string(),
            ));
        }
        let code = i32::from_be_bytes(rest[0..4].try_into().unwrap());
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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourcePacksInfo {
    pub must_accept: bool,
    pub has_scripts: bool,
    pub force_server_packs: bool,
}

impl ResourcePacksInfo {
    pub fn new(must_accept: bool, has_scripts: bool, force_server_packs: bool) -> Self {
        Self {
            must_accept,
            has_scripts,
            force_server_packs,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varuint32(ID_RESOURCE_PACKS_INFO, &mut out);
        out.push(self.must_accept as u8);
        out.push(self.has_scripts as u8);
        out.push(self.force_server_packs as u8);
        out.extend_from_slice(&0u16.to_le_bytes()); // 0 behavior packs
        out.extend_from_slice(&0u16.to_le_bytes()); // 0 resource packs
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let rest = strip_packet_id(bytes, ID_RESOURCE_PACKS_INFO);
        if rest.len() < 3 {
            return Err(BedrockSessionError::MalformedPacket(
                "ResourcePacksInfo too short".to_string(),
            ));
        }
        let must_accept = rest[0] != 0;
        let has_scripts = rest[1] != 0;
        let force_server_packs = rest[2] != 0;
        Ok(Self {
            must_accept,
            has_scripts,
            force_server_packs,
        })
    }
}

/// Response status code sent by client in ResourcePackClientResponse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ResponseStatus {
    None = 0,
    Refused = 1,
    SendPacks = 2,
    HaveAllPacks = 3,
    Completed = 4,
}

impl ResponseStatus {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::None),
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
        out.extend_from_slice(&0u16.to_le_bytes()); // 0 pack entries
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let rest = strip_packet_id(bytes, ID_RESOURCE_PACK_CLIENT_RESPONSE);
        if rest.is_empty() {
            return Err(BedrockSessionError::MalformedPacket(
                "ResourcePackClientResponse empty".to_string(),
            ));
        }
        let status = ResponseStatus::from_u8(rest[0]).ok_or_else(|| {
            BedrockSessionError::MalformedPacket(format!(
                "invalid ResponseStatus byte: {}",
                rest[0]
            ))
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
    pub behavior_packs: Vec<PackInstance>,
    pub resource_packs: Vec<PackInstance>,
}

impl ResourcePackStack {
    pub fn new(must_accept: bool) -> Self {
        Self {
            must_accept,
            behavior_packs: Vec::new(),
            resource_packs: Vec::new(),
        }
    }

    pub fn with_packs(
        must_accept: bool,
        behavior_packs: Vec<PackInstance>,
        resource_packs: Vec<PackInstance>,
    ) -> Self {
        Self {
            must_accept,
            behavior_packs,
            resource_packs,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varuint32(ID_RESOURCE_PACK_STACK, &mut out);
        out.push(self.must_accept as u8);

        put_varuint32(self.behavior_packs.len() as u32, &mut out);
        for pack in &self.behavior_packs {
            put_bedrock_string(&pack.pack_id, &mut out);
            put_bedrock_string(&pack.version, &mut out);
            put_bedrock_string(&pack.sub_pack_name, &mut out);
        }

        put_varuint32(self.resource_packs.len() as u32, &mut out);
        for pack in &self.resource_packs {
            put_bedrock_string(&pack.pack_id, &mut out);
            put_bedrock_string(&pack.version, &mut out);
            put_bedrock_string(&pack.sub_pack_name, &mut out);
        }

        put_bedrock_string(BEDROCK_GAME_VERSION, &mut out);
        out.extend_from_slice(&0u32.to_le_bytes()); // 0 experiments
        out.push(0); // experiments previously toggled = false
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let rest = strip_packet_id(bytes, ID_RESOURCE_PACK_STACK);
        if rest.is_empty() {
            return Err(BedrockSessionError::MalformedPacket(
                "ResourcePackStack empty".to_string(),
            ));
        }
        let must_accept = rest[0] != 0;
        let mut cursor = &rest[1..];

        let mut behavior_packs = Vec::new();
        if let Ok((count, len)) = read_varuint32(cursor) {
            cursor = &cursor[len..];
            for _ in 0..count {
                let (pack_id, n1) = read_bedrock_string(cursor)?;
                cursor = &cursor[n1..];
                let (version, n2) = read_bedrock_string(cursor)?;
                cursor = &cursor[n2..];
                let (sub_pack, n3) = read_bedrock_string(cursor)?;
                cursor = &cursor[n3..];
                behavior_packs.push(PackInstance::new(pack_id, version, sub_pack));
            }
        }

        let mut resource_packs = Vec::new();
        if let Ok((count, len)) = read_varuint32(cursor) {
            cursor = &cursor[len..];
            for _ in 0..count {
                let (pack_id, n1) = read_bedrock_string(cursor)?;
                cursor = &cursor[n1..];
                let (version, n2) = read_bedrock_string(cursor)?;
                cursor = &cursor[n2..];
                let (sub_pack, n3) = read_bedrock_string(cursor)?;
                cursor = &cursor[n3..];
                resource_packs.push(PackInstance::new(pack_id, version, sub_pack));
            }
        }

        Ok(Self {
            must_accept,
            behavior_packs,
            resource_packs,
        })
    }
}

/// Server-to-client world initialization and player entity spawn packet (Packet 0x0B / 11).
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
        out.extend_from_slice(&self.seed.to_le_bytes());
        put_varint32(self.dimension, &mut out);
        put_varint32(self.generator, &mut out);
        put_varint32(self.world_gamemode, &mut out);
        put_varint32(self.difficulty, &mut out);
        put_varint32(self.spawn_pos.0, &mut out);
        put_varint32(self.spawn_pos.1, &mut out);
        put_varint32(self.spawn_pos.2, &mut out);
        put_bedrock_string(&self.level_id, &mut out);
        put_bedrock_string(&self.world_name, &mut out);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockSessionError> {
        let mut rest = strip_packet_id(bytes, ID_START_GAME);
        let (entity_id, n) = read_varint64(rest)?;
        rest = &rest[n..];
        let (runtime_id, n) = read_varuint64(rest)?;
        rest = &rest[n..];
        let (player_gamemode, n) = read_varint32(rest)?;
        rest = &rest[n..];

        if rest.len() < 28 {
            return Err(BedrockSessionError::MalformedPacket(
                "StartGame position and seed too short".to_string(),
            ));
        }
        let px = f32::from_le_bytes(rest[0..4].try_into().unwrap());
        let py = f32::from_le_bytes(rest[4..8].try_into().unwrap());
        let pz = f32::from_le_bytes(rest[8..12].try_into().unwrap());
        let pitch = f32::from_le_bytes(rest[12..16].try_into().unwrap());
        let yaw = f32::from_le_bytes(rest[16..20].try_into().unwrap());
        let seed = i64::from_le_bytes(rest[20..28].try_into().unwrap());
        rest = &rest[28..];

        let (dimension, n) = read_varint32(rest)?;
        rest = &rest[n..];
        let (generator, n) = read_varint32(rest)?;
        rest = &rest[n..];
        let (world_gamemode, n) = read_varint32(rest)?;
        rest = &rest[n..];
        let (difficulty, n) = read_varint32(rest)?;
        rest = &rest[n..];

        let (sx, n) = read_varint32(rest)?;
        rest = &rest[n..];
        let (sy, n) = read_varint32(rest)?;
        rest = &rest[n..];
        let (sz, n) = read_varint32(rest)?;
        rest = &rest[n..];

        let (level_id, n) = read_bedrock_string(rest)?;
        rest = &rest[n..];
        let (world_name, _) = read_bedrock_string(rest)?;

        Ok(Self {
            entity_id,
            runtime_id,
            player_gamemode,
            player_pos: (px, py, pz),
            pitch_yaw: (pitch, yaw),
            seed,
            dimension,
            generator,
            world_gamemode,
            difficulty,
            spawn_pos: (sx, sy, sz),
            level_id: level_id.to_string(),
            world_name: world_name.to_string(),
        })
    }
}

/// Unified enumeration of all supported Bedrock packets in protocol 766.
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
