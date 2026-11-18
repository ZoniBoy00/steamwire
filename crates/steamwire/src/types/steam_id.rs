use std::fmt;
use std::str::FromStr;

use crate::enums::{EAccountType, EUniverse};

/// A Steam ID: a 64-bit value made of the account ID (32 bits), the account
/// instance (20 bits), the account type (4 bits) and the universe (8 bits).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct SteamId(u64);

impl SteamId {
    /// The instance of every desktop user account.
    pub const DESKTOP_INSTANCE: u32 = 1;
    /// The instance mask bit marking a chat ID as a clan chat.
    pub const CHAT_INSTANCE_FLAG_CLAN: u32 = (0x000F_FFFF + 1) >> 1;
    /// The instance mask bit marking a chat ID as a lobby.
    pub const CHAT_INSTANCE_FLAG_LOBBY: u32 = (0x000F_FFFF + 1) >> 2;
    /// The instance mask bit marking a chat ID as an MMS lobby.
    pub const CHAT_INSTANCE_FLAG_MMS_LOBBY: u32 = (0x000F_FFFF + 1) >> 3;

    const INSTANCE_MASK: u32 = 0x000F_FFFF;

    /// Builds a Steam ID from its parts.
    pub const fn new(account_id: u32, instance: u32, universe: EUniverse, account_type: EAccountType) -> Self {
        let raw = (account_id as u64)
            | (((instance & Self::INSTANCE_MASK) as u64) << 32)
            | (((account_type.0 as u64) & 0xF) << 52)
            | (((universe.0 as u64) & 0xFF) << 56);
        Self(raw)
    }

    /// Builds an individual (user) Steam ID in the public universe.
    pub const fn from_account_id(account_id: u32) -> Self {
        Self::new(account_id, Self::DESKTOP_INSTANCE, EUniverse::Public, EAccountType::Individual)
    }

    /// Wraps a raw 64-bit Steam ID.
    pub const fn from_u64(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw 64-bit value.
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// The account ID (low 32 bits).
    pub const fn account_id(self) -> u32 {
        self.0 as u32
    }

    /// The account instance.
    pub const fn instance(self) -> u32 {
        ((self.0 >> 32) as u32) & Self::INSTANCE_MASK
    }

    /// The account type.
    pub const fn account_type(self) -> EAccountType {
        EAccountType(((self.0 >> 52) & 0xF) as i32)
    }

    /// The universe.
    pub const fn universe(self) -> EUniverse {
        EUniverse(((self.0 >> 56) & 0xFF) as i32)
    }

    /// Whether the ID is structurally valid, like `CSteamID::IsValid`.
    pub fn is_valid(self) -> bool {
        let ty = self.account_type();
        if ty.0 <= EAccountType::Invalid.0 || ty.0 > EAccountType::AnonUser.0 {
            return false;
        }
        let universe = self.universe();
        if universe.0 <= EUniverse::Invalid.0 || universe.0 > EUniverse::Dev.0 {
            return false;
        }
        match ty {
            EAccountType::Individual => self.account_id() != 0 && self.instance() <= 4,
            EAccountType::Clan => self.account_id() != 0 && self.instance() == 0,
            EAccountType::GameServer => self.account_id() != 0,
            _ => true,
        }
    }

    /// Whether this is an anonymous (game server or user) account.
    pub fn is_anonymous(self) -> bool {
        matches!(self.account_type(), EAccountType::AnonUser | EAccountType::AnonGameServer)
    }

    /// Whether this is an individual (user) account.
    pub fn is_individual_account(self) -> bool {
        self.account_type() == EAccountType::Individual || self.account_type() == EAccountType::ConsoleUser
    }

    /// Whether this is a clan (group) account.
    pub fn is_clan_account(self) -> bool {
        self.account_type() == EAccountType::Clan
    }

    /// Whether this is a chat account.
    pub fn is_chat_account(self) -> bool {
        self.account_type() == EAccountType::Chat
    }

    /// Whether this is a lobby.
    pub fn is_lobby(self) -> bool {
        self.is_chat_account() && self.instance() & Self::CHAT_INSTANCE_FLAG_LOBBY != 0
    }

    /// Whether this is a game server account.
    pub fn is_game_server_account(self) -> bool {
        matches!(self.account_type(), EAccountType::GameServer | EAccountType::AnonGameServer)
    }

    /// Whether this is a persistent (not anonymous) game server account.
    pub fn is_persistent_game_server_account(self) -> bool {
        self.account_type() == EAccountType::GameServer
    }

    /// Whether this is an anonymous game server account.
    pub fn is_anon_game_server_account(self) -> bool {
        self.account_type() == EAccountType::AnonGameServer
    }

    /// Whether this is an anonymous user account.
    pub fn is_anon_user_account(self) -> bool {
        self.account_type() == EAccountType::AnonUser
    }

    /// Whether this is an anonymous account with no account ID or instance
    /// yet.
    pub fn is_blank_anon_account(self) -> bool {
        self.account_id() == 0 && self.is_anonymous() && self.instance() == 0
    }

    /// Whether this is a content server account.
    pub fn is_content_server_account(self) -> bool {
        self.account_type() == EAccountType::ContentServer
    }

    /// Whether this is a console user account.
    pub fn is_console_user_account(self) -> bool {
        self.account_type() == EAccountType::ConsoleUser
    }

    /// A key that identifies the account whatever its instance (SteamKit2's
    /// `GetStaticAccountKey`).
    pub fn static_account_key(self) -> u64 {
        ((self.universe().0 as u64) << 56) + ((self.account_type().0 as u64) << 52) + self.account_id() as u64
    }

    /// Returns a copy with another instance.
    pub const fn with_instance(self, instance: u32) -> Self {
        Self::new(self.account_id(), instance, self.universe(), self.account_type())
    }

    /// Returns a copy with another account type.
    pub const fn with_account_type(self, account_type: EAccountType) -> Self {
        Self::new(self.account_id(), self.instance(), self.universe(), account_type)
    }

    /// The chat ID of a clan (`ToChatID`); `None` if this isn't a clan.
    pub fn to_chat_id(self) -> Option<Self> {
        self.is_clan_account()
            .then(|| Self::new(self.account_id(), Self::CHAT_INSTANCE_FLAG_CLAN, self.universe(), EAccountType::Chat))
    }

    /// The clan ID of a clan chat; `None` if this isn't a clan chat.
    pub fn to_clan_id(self) -> Option<Self> {
        (self.is_chat_account() && self.instance() & Self::CHAT_INSTANCE_FLAG_CLAN != 0)
            .then(|| Self::new(self.account_id(), 0, self.universe(), EAccountType::Clan))
    }

    /// A chat ID as SteamKit sends it: clan IDs become their chat IDs.
    pub fn as_chat_target(self) -> Self {
        self.to_chat_id().unwrap_or(self)
    }

    /// Renders the ID in the legacy `STEAM_X:Y:Z` form.
    pub fn steam2(self) -> String {
        let universe = match self.universe() {
            EUniverse::Public => 0,
            u => u.0,
        };
        format!("STEAM_{}:{}:{}", universe, self.account_id() & 1, self.account_id() >> 1)
    }

    /// Renders the ID in the `[U:1:123]` form.
    pub fn steam3(self) -> String {
        let ty = self.account_type();
        let mut letter = type_char(ty);
        if ty == EAccountType::Chat {
            if self.instance() & Self::CHAT_INSTANCE_FLAG_CLAN != 0 {
                letter = 'c';
            } else if self.instance() & Self::CHAT_INSTANCE_FLAG_LOBBY != 0 {
                letter = 'L';
            }
        }

        let show_instance = match ty {
            EAccountType::AnonGameServer | EAccountType::Multiseat => true,
            EAccountType::Individual => self.instance() != Self::DESKTOP_INSTANCE,
            _ => false,
        };

        if show_instance {
            format!("[{}:{}:{}:{}]", letter, self.universe().0, self.account_id(), self.instance())
        } else {
            format!("[{}:{}:{}]", letter, self.universe().0, self.account_id())
        }
    }

    /// Parses a `STEAM_X:Y:Z` ID.
    pub fn parse_steam2(s: &str) -> Option<Self> {
        let rest = s.trim().strip_prefix("STEAM_")?;
        let mut parts = rest.split(':');
        let universe: i32 = parts.next()?.parse().ok()?;
        let auth_server: u32 = parts.next()?.parse().ok()?;
        let account: u32 = parts.next()?.parse().ok()?;
        if parts.next().is_some() || auth_server > 1 {
            return None;
        }
        let universe = if universe == 0 { EUniverse::Public } else { EUniverse(universe) };
        Some(Self::new((account << 1) | auth_server, Self::DESKTOP_INSTANCE, universe, EAccountType::Individual))
    }

    /// Parses a `[U:1:123]` / `[U:1:123:4]` ID (the brackets are optional).
    pub fn parse_steam3(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('[').unwrap_or(s);
        let s = s.strip_suffix(']').unwrap_or(s);
        let mut parts = s.split(':');
        let letter = {
            let p = parts.next()?;
            let mut chars = p.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            c
        };
        let universe: i32 = parts.next()?.parse().ok()?;
        let account: u32 = parts.next()?.parse().ok()?;
        let explicit_instance: Option<u32> = match parts.next() {
            Some(p) => Some(p.parse().ok()?),
            None => None,
        };
        if parts.next().is_some() {
            return None;
        }

        let (ty, mut instance) = match letter {
            'I' => (EAccountType::Invalid, 0),
            'U' => (EAccountType::Individual, Self::DESKTOP_INSTANCE),
            'M' => (EAccountType::Multiseat, 0),
            'G' => (EAccountType::GameServer, 0),
            'A' => (EAccountType::AnonGameServer, 0),
            'P' => (EAccountType::Pending, 0),
            'C' => (EAccountType::ContentServer, 0),
            'g' => (EAccountType::Clan, 0),
            'T' => (EAccountType::Chat, 0),
            'c' => (EAccountType::Chat, Self::CHAT_INSTANCE_FLAG_CLAN),
            'L' => (EAccountType::Chat, Self::CHAT_INSTANCE_FLAG_LOBBY),
            'a' => (EAccountType::AnonUser, 0),
            _ => return None,
        };
        if let Some(i) = explicit_instance {
            instance = i;
        }
        Some(Self::new(account, instance, EUniverse(universe), ty))
    }
}

fn type_char(ty: EAccountType) -> char {
    match ty {
        EAccountType::Individual => 'U',
        EAccountType::Multiseat => 'M',
        EAccountType::GameServer => 'G',
        EAccountType::AnonGameServer => 'A',
        EAccountType::Pending => 'P',
        EAccountType::ContentServer => 'C',
        EAccountType::Clan => 'g',
        EAccountType::Chat => 'T',
        EAccountType::AnonUser => 'a',
        _ => 'I',
    }
}

impl From<u64> for SteamId {
    fn from(raw: u64) -> Self {
        Self(raw)
    }
}

impl From<SteamId> for u64 {
    fn from(id: SteamId) -> Self {
        id.0
    }
}

impl fmt::Debug for SteamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SteamId({} {})", self.0, self.steam3())
    }
}

impl fmt::Display for SteamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.steam3())
    }
}

/// Error from parsing a [`SteamId`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid Steam ID")]
pub struct ParseSteamIdError;

impl FromStr for SteamId {
    type Err = ParseSteamIdError;

    /// Accepts a 64-bit number, `STEAM_X:Y:Z` or `[U:1:123]`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Ok(raw) = s.parse::<u64>() {
            return Ok(Self(raw));
        }
        Self::parse_steam2(s).or_else(|| Self::parse_steam3(s)).ok_or(ParseSteamIdError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_round_trip() {
        let id = SteamId::from_u64(76561197960287930);
        assert_eq!(id.account_id(), 22202);
        assert_eq!(id.instance(), 1);
        assert_eq!(id.account_type(), EAccountType::Individual);
        assert_eq!(id.universe(), EUniverse::Public);
        assert_eq!(id, SteamId::from_account_id(22202));
        assert!(id.is_valid());
    }

    /// Values from SteamKit2's `SteamID.GetStaticAccountKey`.
    #[test]
    fn static_account_key() {
        assert_eq!(SteamId::from_u64(76561197960287930).static_account_key(), 76561193665320634);
        assert_eq!(SteamId::from_u64(103582791429521412).static_account_key(), 103582791429521412);
    }

    #[test]
    fn render_and_parse() {
        let id = SteamId::from_u64(76561197960287930);
        assert_eq!(id.steam2(), "STEAM_0:0:11101");
        assert_eq!(id.steam3(), "[U:1:22202]");
        assert_eq!("STEAM_0:0:11101".parse::<SteamId>().unwrap(), id);
        assert_eq!("[U:1:22202]".parse::<SteamId>().unwrap(), id);
        assert_eq!("76561197960287930".parse::<SteamId>().unwrap(), id);
    }

    #[test]
    fn clan_chat_and_anon() {
        let clan = "[g:1:4]".parse::<SteamId>().unwrap();
        assert_eq!(clan.account_type(), EAccountType::Clan);
        assert_eq!(clan.as_u64(), 103582791429521412);

        let lobby = "[L:1:123]".parse::<SteamId>().unwrap();
        assert_eq!(lobby.steam3(), "[L:1:123]");

        let anon = SteamId::new(0, 0, EUniverse::Public, EAccountType::AnonUser);
        assert!(anon.is_anonymous());
        assert_eq!(anon.steam3(), "[a:1:0]");
        assert!("garbage".parse::<SteamId>().is_err());
    }
}
