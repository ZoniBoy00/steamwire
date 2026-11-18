//! A raw LZMA decoder (SteamKit2's `LzmaDecoder`). It decodes straight into
//! the output buffer, so the dictionary size is not needed, and a match
//! running past the end of the output is cut short.

use crate::error::{Error, Result};

/// The size of the LZMA properties (`lc/lp/pb` byte and dictionary size).
pub const PROPERTIES_SIZE: usize = 5;

const STATE_COUNT: usize = 12;
const LITERAL_STATE_COUNT: usize = 7;
const POS_STATE_BITS_MAX: usize = 4;
const LITERAL_CODER_SIZE: usize = 0x300;
const END_POS_MODEL_INDEX: u32 = 14;
const FULL_DISTANCE_COUNT: usize = 1 << (END_POS_MODEL_INDEX >> 1);
const ALIGN_BITS: u32 = 4;

const LENGTH_CHOICE: usize = 0;
const LENGTH_CHOICE2: usize = 1;
const LENGTH_LOW: usize = 2;
const LENGTH_MID: usize = LENGTH_LOW + (8 << POS_STATE_BITS_MAX);
const LENGTH_HIGH: usize = LENGTH_MID + (8 << POS_STATE_BITS_MAX);
const LENGTH_CODER_SIZE: usize = LENGTH_HIGH + 256;

// Offsets into the single probability array.
const IS_MATCH: usize = 0;
const IS_REP: usize = IS_MATCH + (STATE_COUNT << POS_STATE_BITS_MAX);
const IS_REP_G0: usize = IS_REP + STATE_COUNT;
const IS_REP_G1: usize = IS_REP_G0 + STATE_COUNT;
const IS_REP_G2: usize = IS_REP_G1 + STATE_COUNT;
const IS_REP0_LONG: usize = IS_REP_G2 + STATE_COUNT;
const POS_SLOT: usize = IS_REP0_LONG + (STATE_COUNT << POS_STATE_BITS_MAX);
const SPEC_POS: usize = POS_SLOT + (4 << 6);
const ALIGN: usize = SPEC_POS + FULL_DISTANCE_COUNT - END_POS_MODEL_INDEX as usize;
const MATCH_LENGTH: usize = ALIGN + (1 << ALIGN_BITS);
const REP_LENGTH: usize = MATCH_LENGTH + LENGTH_CODER_SIZE;
const LITERAL: usize = REP_LENGTH + LENGTH_CODER_SIZE;

const BIT_MODEL_TOTAL_BITS: u32 = 11;
const MOVE_BITS: u32 = 5;
const TOP_VALUE: u32 = 1 << 24;
const INITIAL_PROBABILITY: u16 = 1 << (BIT_MODEL_TOTAL_BITS - 1);

fn invalid(msg: &str) -> Error {
    Error::InvalidData(format!("LZMA: {msg}"))
}

struct RangeDecoder<'a> {
    input: &'a [u8],
    pos: usize,
    range: u32,
    code: u32,
}

impl<'a> RangeDecoder<'a> {
    fn new(input: &'a [u8]) -> Result<Self> {
        if input.len() < 5 {
            return Err(invalid("stream is truncated"));
        }
        // The first byte is always 0 and is skipped.
        let code = u32::from_be_bytes(input[1..5].try_into().unwrap());
        Ok(Self { input, pos: 5, range: u32::MAX, code })
    }

    fn normalize(&mut self) -> Result<()> {
        if self.range < TOP_VALUE {
            let byte = *self.input.get(self.pos).ok_or_else(|| invalid("stream is truncated"))?;
            self.pos += 1;
            self.range <<= 8;
            self.code = (self.code << 8) | byte as u32;
        }
        Ok(())
    }

    fn bit(&mut self, prob: &mut u16) -> Result<u32> {
        let p = *prob as u32;
        let bound = (self.range >> BIT_MODEL_TOTAL_BITS) * p;
        let bit = if self.code < bound {
            self.range = bound;
            *prob = (p + (((1 << BIT_MODEL_TOTAL_BITS) - p) >> MOVE_BITS)) as u16;
            0
        } else {
            self.range -= bound;
            self.code -= bound;
            *prob = (p - (p >> MOVE_BITS)) as u16;
            1
        };
        self.normalize()?;
        Ok(bit)
    }

    fn bit_tree(&mut self, probs: &mut [u16], bits: u32) -> Result<u32> {
        let mut symbol = 1u32;
        for _ in 0..bits {
            symbol = (symbol << 1) | self.bit(&mut probs[symbol as usize])?;
        }
        Ok(symbol - (1 << bits))
    }

    fn reverse_bit_tree(&mut self, probs: &mut [u16], bits: u32) -> Result<u32> {
        let (mut index, mut symbol) = (1usize, 0u32);
        for i in 0..bits {
            let bit = self.bit(&mut probs[index])?;
            index = (index << 1) | bit as usize;
            symbol |= bit << i;
        }
        Ok(symbol)
    }

    fn direct_bits(&mut self, bits: u32) -> Result<u32> {
        let mut result = 0u32;
        for _ in 0..bits {
            self.range >>= 1;
            let bit = if self.code >= self.range {
                self.code -= self.range;
                1
            } else {
                0
            };
            result = (result << 1) | bit;
            self.normalize()?;
        }
        Ok(result)
    }

    fn literal(&mut self, probs: &mut [u16]) -> Result<u8> {
        let mut symbol = 1u32;
        while symbol < 0x100 {
            symbol = (symbol << 1) | self.bit(&mut probs[symbol as usize])?;
        }
        Ok(symbol as u8)
    }

    fn matched_literal(&mut self, probs: &mut [u16], mut match_byte: u32) -> Result<u8> {
        let mut symbol = 1u32;
        // While decoded bits agree with the match byte, each bit uses the
        // probabilities selected by the match bit; after the first
        // difference, the plain ones.
        loop {
            match_byte <<= 1;
            let match_bit = (match_byte >> 8) & 1;
            let bit = self.bit(&mut probs[(0x100 + (match_bit << 8) + symbol) as usize])?;
            symbol = (symbol << 1) | bit;
            if symbol >= 0x100 {
                return Ok(symbol as u8);
            }
            if bit != match_bit {
                break;
            }
        }
        while symbol < 0x100 {
            symbol = (symbol << 1) | self.bit(&mut probs[symbol as usize])?;
        }
        Ok(symbol as u8)
    }

    fn length(&mut self, probs: &mut [u16], pos_state: usize) -> Result<u32> {
        if self.bit(&mut probs[LENGTH_CHOICE])? == 0 {
            return self.bit_tree(&mut probs[LENGTH_LOW + (pos_state << 3)..], 3);
        }
        if self.bit(&mut probs[LENGTH_CHOICE2])? == 0 {
            return Ok(8 + self.bit_tree(&mut probs[LENGTH_MID + (pos_state << 3)..], 3)?);
        }
        Ok(16 + self.bit_tree(&mut probs[LENGTH_HIGH..], 8)?)
    }

    fn distance(&mut self, probs: &mut [u16], length: u32) -> Result<u32> {
        let length_state = length.min(3) as usize;
        let pos_slot = self.bit_tree(&mut probs[POS_SLOT + (length_state << 6)..], 6)?;
        if pos_slot < 4 {
            return Ok(pos_slot);
        }
        let direct_bits = (pos_slot >> 1) - 1;
        let distance = (2 | (pos_slot & 1)) << direct_bits;
        if pos_slot < END_POS_MODEL_INDEX {
            // These slots' trees are packed back to back, each starting one
            // before its first used index.
            let tree = SPEC_POS + distance as usize - pos_slot as usize - 1;
            return Ok(distance + self.reverse_bit_tree(&mut probs[tree..], direct_bits)?);
        }
        let distance = distance + (self.direct_bits(direct_bits - ALIGN_BITS)? << ALIGN_BITS);
        Ok(distance + self.reverse_bit_tree(&mut probs[ALIGN..], ALIGN_BITS)?)
    }
}

fn state_after_literal(state: usize) -> usize {
    match state {
        0..=3 => 0,
        4..=9 => state - 3,
        _ => state - 6,
    }
}

/// Decodes a raw LZMA stream until `output` is full.
pub fn decode(properties: &[u8], input: &[u8], output: &mut [u8]) -> Result<()> {
    decode_with_dictionary(properties, input, output, 0)
}

/// Decodes a raw LZMA stream after the first `preset` bytes of `output`,
/// which matches can refer to (a preset dictionary, as in VZip deltas).
/// Positions count from the start of `output`, like SteamKit2's decoder.
pub fn decode_with_dictionary(properties: &[u8], input: &[u8], output: &mut [u8], preset: usize) -> Result<()> {
    if preset > output.len() {
        return Err(invalid("preset dictionary is larger than the output"));
    }
    if properties.len() < PROPERTIES_SIZE {
        return Err(invalid("properties are too short"));
    }
    let props = properties[0] as usize;
    if props >= 9 * 5 * 5 {
        return Err(invalid("invalid properties"));
    }
    let (lc, lp, pb) = (props % 9, props / 9 % 5, props / 45);
    let mut probs = vec![INITIAL_PROBABILITY; LITERAL + (LITERAL_CODER_SIZE << (lc + lp))];
    let mut rc = RangeDecoder::new(input)?;

    let pos_state_mask = (1 << pb) - 1;
    let literal_pos_mask = (1 << lp) - 1;
    let (mut state, mut rep0, mut rep1, mut rep2, mut rep3) = (0usize, 0u32, 0u32, 0u32, 0u32);
    let mut pos = preset;

    while pos < output.len() {
        let pos_state = pos & pos_state_mask;
        let state_index = (state << POS_STATE_BITS_MAX) + pos_state;

        if rc.bit(&mut probs[IS_MATCH + state_index])? == 0 {
            let previous = if pos > 0 { output[pos - 1] as usize } else { 0 };
            let literal_state = ((pos & literal_pos_mask) << lc) + (previous >> (8 - lc));
            let base = LITERAL + LITERAL_CODER_SIZE * literal_state;
            let literal_probs = &mut probs[base..base + LITERAL_CODER_SIZE];
            output[pos] = if state < LITERAL_STATE_COUNT {
                rc.literal(literal_probs)?
            } else {
                let match_byte = output[pos - rep0 as usize - 1] as u32;
                rc.matched_literal(literal_probs, match_byte)?
            };
            pos += 1;
            state = state_after_literal(state);
            continue;
        }

        let length;
        if rc.bit(&mut probs[IS_REP + state])? == 0 {
            rep3 = rep2;
            rep2 = rep1;
            rep1 = rep0;
            length = rc.length(&mut probs[MATCH_LENGTH..], pos_state)?;
            state = if state < LITERAL_STATE_COUNT { 7 } else { 10 };
            rep0 = rc.distance(&mut probs, length)?;
            if rep0 == u32::MAX {
                // An end marker: decoding stops at the output size first.
                return Err(invalid("stream ended before the expected size"));
            }
            if rep0 as usize >= pos {
                return Err(invalid("match distance is outside of the decoded data"));
            }
        } else {
            if pos == 0 {
                return Err(invalid("stream starts with a repeated match"));
            }
            if rc.bit(&mut probs[IS_REP_G0 + state])? == 0 {
                if rc.bit(&mut probs[IS_REP0_LONG + state_index])? == 0 {
                    output[pos] = output[pos - rep0 as usize - 1];
                    pos += 1;
                    state = if state < LITERAL_STATE_COUNT { 9 } else { 11 };
                    continue;
                }
            } else {
                let distance;
                if rc.bit(&mut probs[IS_REP_G1 + state])? == 0 {
                    distance = rep1;
                } else {
                    if rc.bit(&mut probs[IS_REP_G2 + state])? == 0 {
                        distance = rep2;
                    } else {
                        distance = rep3;
                        rep3 = rep2;
                    }
                    rep2 = rep1;
                }
                rep1 = rep0;
                rep0 = distance;
            }
            length = rc.length(&mut probs[REP_LENGTH..], pos_state)?;
            state = if state < LITERAL_STATE_COUNT { 8 } else { 11 };
        }

        // A match running past the end of the output is cut short.
        let match_len = (length as usize + 2).min(output.len() - pos);
        let distance = rep0 as usize + 1;
        if distance > pos {
            return Err(invalid("match distance is outside of the decoded data"));
        }
        // Overlapping matches copy bytes they produce, so go forwards.
        for i in pos..pos + match_len {
            output[i] = output[i - distance];
        }
        pos += match_len;
    }
    Ok(())
}
