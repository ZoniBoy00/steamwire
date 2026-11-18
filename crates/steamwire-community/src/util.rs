//! Steam's JSON writes 64-bit numbers as strings, and sometimes small ones
//! too.

use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serde(untagged)]
enum Number {
    Int(u64),
    Text(String),
}

fn parse<E: serde::de::Error>(number: Number) -> Result<u64, E> {
    match number {
        Number::Int(n) => Ok(n),
        Number::Text(text) => text.trim().parse().map_err(|_| E::custom(format!("not a number: {text:?}"))),
    }
}

/// A `u64` written as a number or a string.
pub(crate) fn flexible_u64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    parse(Number::deserialize(deserializer)?)
}

/// An optional `u64` written as a number or a string; `null` and `""` are
/// `None`.
pub(crate) fn optional_flexible_u64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    match Option::<Number>::deserialize(deserializer)? {
        None => Ok(None),
        Some(Number::Text(text)) if text.is_empty() => Ok(None),
        Some(number) => parse(number).map(Some),
    }
}

/// A flag written as `true`, `1` or `"1"`.
pub(crate) fn flexible_bool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Flag {
        Bool(bool),
        Int(i64),
        Text(String),
    }
    Ok(match Flag::deserialize(deserializer)? {
        Flag::Bool(b) => b,
        Flag::Int(n) => n != 0,
        Flag::Text(t) => !matches!(t.as_str(), "" | "0" | "false"),
    })
}
