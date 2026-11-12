//! Java Edition 26.3 (protocol 777) packet identifiers used by RustMC.
//!
//! Every value comes from the packet report produced by the official 26.3
//! server's data generator (`java -DbundlerMainClass=net.minecraft.data.Main
//! -jar server.jar --reports`), recorded in `docs/PROVENANCE.md`. The report
//! itself is operator-local and is not committed. Only packets RustMC reads or
//! writes are listed; an identifier here is not a claim that the packet's
//! gameplay behavior is implemented.

/// `handshake` phase.
pub mod handshake {
    pub mod serverbound {
        pub const INTENTION: u32 = 0;
    }
}

/// Handshake `intention` next-state values.
pub mod intent {
    pub const STATUS: u32 = 1;
    pub const LOGIN: u32 = 2;
}

/// `status` phase.
pub mod status {
    pub mod serverbound {
        pub const STATUS_REQUEST: u32 = 0;
        pub const PING_REQUEST: u32 = 1;
    }
    pub mod clientbound {
        pub const STATUS_RESPONSE: u32 = 0;
        pub const PONG_RESPONSE: u32 = 1;
    }
}

/// `login` phase.
pub mod login {
    pub mod serverbound {
        pub const HELLO: u32 = 0;
        pub const LOGIN_ACKNOWLEDGED: u32 = 3;
    }
    pub mod clientbound {
        pub const LOGIN_DISCONNECT: u32 = 0;
        pub const LOGIN_FINISHED: u32 = 2;
    }
}

/// `configuration` phase.
pub mod configuration {
    pub mod serverbound {
        pub const CLIENT_INFORMATION: u32 = 0;
        pub const CUSTOM_PAYLOAD: u32 = 2;
        pub const FINISH_CONFIGURATION: u32 = 3;
        pub const SELECT_KNOWN_PACKS: u32 = 7;
    }
    pub mod clientbound {
        pub const FINISH_CONFIGURATION: u32 = 3;
        pub const REGISTRY_DATA: u32 = 7;
        pub const UPDATE_ENABLED_FEATURES: u32 = 13;
        pub const UPDATE_TAGS: u32 = 14;
        pub const SELECT_KNOWN_PACKS: u32 = 15;
    }
}

/// `play` phase.
pub mod play {
    pub mod serverbound {
        pub const ACCEPT_TELEPORTATION: u32 = 0;
        pub const CHUNK_BATCH_RECEIVED: u32 = 11;
        pub const CLIENT_TICK_END: u32 = 13;
        pub const CLIENT_INFORMATION: u32 = 14;
        pub const CUSTOM_PAYLOAD: u32 = 22;
        pub const KEEP_ALIVE: u32 = 28;
        pub const MOVE_PLAYER_POS: u32 = 30;
        pub const MOVE_PLAYER_POS_ROT: u32 = 31;
        pub const MOVE_PLAYER_ROT: u32 = 32;
        pub const MOVE_PLAYER_STATUS_ONLY: u32 = 33;
        pub const PLAYER_ABILITIES: u32 = 40;
        pub const PLAYER_ACTION: u32 = 41;
        pub const PLAYER_COMMAND: u32 = 42;
        pub const PLAYER_INPUT: u32 = 43;
        pub const PLAYER_LOADED: u32 = 44;
        pub const PUNCH: u32 = 46;
        pub const SET_CARRIED_ITEM: u32 = 54;
        pub const SET_CREATIVE_MODE_SLOT: u32 = 57;
        pub const SPECTATOR_ACTION: u32 = 63;
        pub const USE_ITEM_ON: u32 = 66;
        pub const USE_ITEM: u32 = 67;
    }
    pub mod clientbound {
        pub const CHUNK_BATCH_FINISHED: u32 = 11;
        pub const CHUNK_BATCH_START: u32 = 12;
        pub const FORGET_LEVEL_CHUNK: u32 = 38;
        pub const GAME_EVENT: u32 = 39;
        pub const KEEP_ALIVE: u32 = 45;
        pub const LEVEL_CHUNK_WITH_LIGHT: u32 = 46;
        pub const LOGIN: u32 = 50;
        pub const PLAYER_ABILITIES: u32 = 65;
        pub const PLAYER_POSITION: u32 = 73;
        pub const SET_CHUNK_CACHE_CENTER: u32 = 96;
        pub const SET_TIME: u32 = 115;
    }
}
