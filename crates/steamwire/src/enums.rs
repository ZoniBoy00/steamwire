//! Steam enums generated from SteamKit2's SteamLanguage files (`steamd/`).
//!
//! Each enum is a newtype over its wire type with one associated constant per
//! member, e.g. [`EMsg::ClientLogon`] or [`EResult::OK`]. Unknown values are
//! kept as-is; `name()` returns `None` for them.

#![allow(missing_docs, clippy::all)]

/// What every generated enum can do: look a member up by name or value.
pub trait SteamEnum: Sized + Copy {
    /// The member called `name`.
    fn from_name(name: &str) -> Option<Self>;
    /// The value `value`, if it fits the enum's wire type.
    fn from_i64(value: i64) -> Option<Self>;
}

include!(concat!(env!("OUT_DIR"), "/enums.rs"));
