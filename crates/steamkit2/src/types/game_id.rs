use std::fmt;

/// The type of a [`GameId`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GameType {
    /// A Steam application.
    App = 0,
    /// A game mod.
    GameMod = 1,
    /// A shortcut to a non-Steam game.
    Shortcut = 2,
    /// A peer-to-peer file.
    P2P = 3,
}

/// A game ID: the app ID (24 bits), the game type (8 bits) and the mod ID
/// (32 bits).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct GameId(u64);

impl GameId {
    /// The game ID of an application.
    pub const fn from_app_id(app_id: u32) -> Self {
        Self((app_id & 0x00FF_FFFF) as u64)
    }

    /// Wraps a raw value.
    pub const fn from_u64(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw value.
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// The app ID.
    pub const fn app_id(self) -> u32 {
        (self.0 & 0x00FF_FFFF) as u32
    }

    /// The game type.
    pub const fn game_type(self) -> GameType {
        match (self.0 >> 24) & 0xFF {
            1 => GameType::GameMod,
            2 => GameType::Shortcut,
            3 => GameType::P2P,
            _ => GameType::App,
        }
    }

    /// The mod ID.
    pub const fn mod_id(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// The game ID of a mod of `app_id`, from the mod's path (SteamKit2's
    /// `GameID(uint, string)`).
    pub fn from_mod(app_id: u32, mod_path: &str) -> Self {
        Self::new(app_id, GameType::GameMod, crc32fast::hash(mod_path.as_bytes()) | 0x8000_0000)
    }

    /// The game ID of a shortcut to a non-Steam game (SteamKit2's
    /// `GameID(string, string)`).
    pub fn from_shortcut(exe_path: &str, app_name: &str) -> Self {
        let combined = [exe_path, app_name].concat();
        Self::new(0, GameType::Shortcut, crc32fast::hash(combined.as_bytes()) | 0x8000_0000)
    }

    /// Whether this is a mod.
    pub const fn is_mod(self) -> bool {
        matches!(self.game_type(), GameType::GameMod)
    }

    /// Whether this is a shortcut to a non-Steam game.
    pub const fn is_shortcut(self) -> bool {
        matches!(self.game_type(), GameType::Shortcut)
    }

    /// Whether this is a peer-to-peer file.
    pub const fn is_p2p_file(self) -> bool {
        matches!(self.game_type(), GameType::P2P)
    }

    /// Whether this is a Steam application.
    pub const fn is_steam_app(self) -> bool {
        matches!(self.game_type(), GameType::App)
    }

    /// Builds a mod or shortcut game ID.
    pub const fn new(app_id: u32, game_type: GameType, mod_id: u32) -> Self {
        Self((app_id & 0x00FF_FFFF) as u64 | ((game_type as u64) << 24) | ((mod_id as u64) << 32))
    }
}

impl fmt::Debug for GameId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GameId")
            .field("app_id", &self.app_id())
            .field("type", &self.game_type())
            .field("mod_id", &self.mod_id())
            .finish()
    }
}

impl fmt::Display for GameId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mod_round_trip() {
        let id = GameId::new(215, GameType::GameMod, 0x8000_1234);
        assert_eq!(id.app_id(), 215);
        assert_eq!(id.game_type(), GameType::GameMod);
        assert_eq!(id.mod_id(), 0x8000_1234);
        assert_eq!(GameId::from_app_id(440).as_u64(), 440);
    }

    /// Values from SteamKit2's `GameID` constructors.
    #[test]
    fn mods_and_shortcuts_match_steamkit2() {
        let tf = GameId::from_mod(440, "tf");
        assert_eq!(tf.as_u64(), 12563752546046312888);
        assert!(tf.is_mod() && !tf.is_steam_app());
        assert_eq!(GameId::from_mod(215, "C:/Games/mymod").as_u64(), 18143706050497347799);

        let shortcut = GameId::from_shortcut("C:/Program Files/Game/game.exe", "My Game");
        assert_eq!(shortcut.as_u64(), 15060988996022697984);
        assert!(shortcut.is_shortcut());
        assert_eq!(shortcut.app_id(), 0);
        assert_eq!(GameId::from_shortcut("/home/ä/peli", "Pelí").as_u64(), 11684385046206087168);
        assert!(GameId::from_app_id(440).is_steam_app());
    }
}
