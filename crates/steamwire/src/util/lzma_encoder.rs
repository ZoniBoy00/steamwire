//! A raw LZMA encoder: a port of the LZMA SDK's C# encoder that SteamKit2
//! uses for VZip (`SevenZip.Compression.LZMA.Encoder` and its `BinTree`
//! match finder), so it produces the same bytes. The arithmetic keeps the
//! C# code's wrapping `uint` semantics.

use super::lzma::PROPERTIES_SIZE;

const NUM_REP_DISTANCES: u32 = 4;
const NUM_STATES: usize = 12;
const NUM_POS_SLOT_BITS: u32 = 6;
const NUM_LEN_TO_POS_STATES_BITS: u32 = 2;
const NUM_LEN_TO_POS_STATES: u32 = 1 << NUM_LEN_TO_POS_STATES_BITS;
const MATCH_MIN_LEN: u32 = 2;
const NUM_ALIGN_BITS: u32 = 4;
const ALIGN_TABLE_SIZE: u32 = 1 << NUM_ALIGN_BITS;
const ALIGN_MASK: u32 = ALIGN_TABLE_SIZE - 1;
const START_POS_MODEL_INDEX: u32 = 4;
const END_POS_MODEL_INDEX: u32 = 14;
const NUM_FULL_DISTANCES: u32 = 1 << (END_POS_MODEL_INDEX / 2);
const NUM_POS_STATES_BITS_MAX: u32 = 4;
const NUM_POS_STATES_ENCODING_MAX: usize = 1 << 4;
const NUM_LOW_LEN_BITS: u32 = 3;
const NUM_MID_LEN_BITS: u32 = 3;
const NUM_HIGH_LEN_BITS: u32 = 8;
const NUM_LOW_LEN_SYMBOLS: u32 = 1 << NUM_LOW_LEN_BITS;
const NUM_MID_LEN_SYMBOLS: u32 = 1 << NUM_MID_LEN_BITS;
const NUM_LEN_SYMBOLS: u32 = NUM_LOW_LEN_SYMBOLS + NUM_MID_LEN_SYMBOLS + (1 << NUM_HIGH_LEN_BITS);
const MATCH_MAX_LEN: u32 = MATCH_MIN_LEN + NUM_LEN_SYMBOLS - 1;

const INFINITY_PRICE: u32 = 0xFFF_FFFF;
const NUM_OPTS: u32 = 1 << 12;

const BIT_MODEL_TOTAL_BITS: u32 = 11;
const BIT_MODEL_TOTAL: u32 = 1 << BIT_MODEL_TOTAL_BITS;
const NUM_MOVE_BITS: u32 = 5;
const NUM_MOVE_REDUCING_BITS: u32 = 2;
const NUM_BIT_PRICE_SHIFT_BITS: u32 = 6;
const TOP_VALUE: u32 = 1 << 24;

/// The match finder (the SDK's `BT2` or `BT4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchFinder {
    /// Binary tree with 2-byte hashing.
    Bt2,
    /// Binary tree with 2-, 3- and 4-byte hashing.
    Bt4,
}

/// Encoder settings, as passed to the SDK's `SetCoderProperties`.
#[derive(Clone, Debug)]
pub struct EncoderOptions {
    /// The dictionary size, at most 2^30.
    pub dictionary_size: u32,
    /// `pb`, 0 to 4.
    pub pos_state_bits: u32,
    /// `lc`, 0 to 8.
    pub literal_context_bits: u32,
    /// `lp`, 0 to 4.
    pub literal_pos_bits: u32,
    /// The number of fast bytes, 5 to 273.
    pub fast_bytes: u32,
    /// The match finder.
    pub match_finder: MatchFinder,
    /// Whether to end the stream with an end marker.
    pub end_marker: bool,
}

impl EncoderOptions {
    /// The settings SteamKit2's `VZipUtil.Compress` uses.
    pub const VZIP: Self = Self {
        dictionary_size: 1 << 23,
        pos_state_bits: 2,
        literal_context_bits: 3,
        literal_pos_bits: 0,
        fast_bytes: 128,
        match_finder: MatchFinder::Bt4,
        end_marker: false,
    };

    fn validate(&self) -> Option<()> {
        let valid = (1..=1 << 30).contains(&self.dictionary_size)
            && self.pos_state_bits <= 4
            && self.literal_context_bits <= 8
            && self.literal_pos_bits <= 4
            && (5..=MATCH_MAX_LEN).contains(&self.fast_bytes);
        valid.then_some(())
    }

    /// The 5-byte properties header: `(pb * 5 + lp) * 9 + lc` and the
    /// dictionary size.
    pub fn properties(&self) -> [u8; PROPERTIES_SIZE] {
        let mut props = [0u8; PROPERTIES_SIZE];
        props[0] = ((self.pos_state_bits * 5 + self.literal_pos_bits) * 9 + self.literal_context_bits) as u8;
        props[1..].copy_from_slice(&self.dictionary_size.to_le_bytes());
        props
    }
}

/// Compresses `input` and returns the properties header followed by the raw
/// LZMA stream (the SDK's `WriteCoderProperties` then `Code`). `None` if the
/// options are out of range.
pub fn encode(input: &[u8], options: &EncoderOptions) -> Option<Vec<u8>> {
    options.validate()?;
    let mut out = options.properties().to_vec();
    let mut encoder = Encoder::new(options, &mut out);
    let mut mf = BinTree::new(options, input);
    encoder.code(&mut mf);
    Some(out)
}

// ------------------------------------------------------------------ tables

/// `CRC.Table` from the SDK, used by the match finder's hashes.
static CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut r = i as u32;
        let mut j = 0;
        while j < 8 {
            r = if r & 1 != 0 { (r >> 1) ^ 0xEDB8_8320 } else { r >> 1 };
            j += 1;
        }
        table[i] = r;
        i += 1;
    }
    table
};

/// `BitEncoder.ProbPrices`.
static PROB_PRICES: [u32; (BIT_MODEL_TOTAL >> NUM_MOVE_REDUCING_BITS) as usize] = {
    let mut prices = [0u32; (BIT_MODEL_TOTAL >> NUM_MOVE_REDUCING_BITS) as usize];
    const NUM_BITS: u32 = BIT_MODEL_TOTAL_BITS - NUM_MOVE_REDUCING_BITS;
    let mut i = NUM_BITS as i32 - 1;
    while i >= 0 {
        let start = 1u32 << (NUM_BITS - i as u32 - 1);
        let end = 1u32 << (NUM_BITS - i as u32);
        let mut j = start;
        while j < end {
            prices[j as usize] = ((i as u32) << NUM_BIT_PRICE_SHIFT_BITS)
                + (((end - j) << NUM_BIT_PRICE_SHIFT_BITS) >> (NUM_BITS - i as u32 - 1));
            j += 1;
        }
        i -= 1;
    }
    prices
};

/// `Encoder.g_FastPos`.
static FAST_POS: [u8; 1 << 11] = {
    let mut fast = [0u8; 1 << 11];
    fast[1] = 1;
    let mut c = 2;
    let mut slot = 2u8;
    while slot < 22 {
        let k = 1u32 << ((slot >> 1) - 1);
        let mut j = 0;
        while j < k {
            fast[c] = slot;
            c += 1;
            j += 1;
        }
        slot += 1;
    }
    fast
};

fn pos_slot(pos: u32) -> u32 {
    if pos < 1 << 11 {
        FAST_POS[pos as usize] as u32
    } else if pos < 1 << 21 {
        FAST_POS[(pos >> 10) as usize] as u32 + 20
    } else {
        FAST_POS[(pos >> 20) as usize] as u32 + 40
    }
}

fn pos_slot2(pos: u32) -> u32 {
    if pos < 1 << 17 {
        FAST_POS[(pos >> 6) as usize] as u32 + 12
    } else if pos < 1 << 27 {
        FAST_POS[(pos >> 16) as usize] as u32 + 32
    } else {
        FAST_POS[(pos >> 26) as usize] as u32 + 52
    }
}

fn len_to_pos_state(len: u32) -> u32 {
    let len = len - MATCH_MIN_LEN;
    if len < NUM_LEN_TO_POS_STATES {
        len
    } else {
        NUM_LEN_TO_POS_STATES - 1
    }
}

// ----------------------------------------------------------- range coder

struct RangeEncoder<'a> {
    out: &'a mut Vec<u8>,
    low: u64,
    range: u32,
    cache_size: u32,
    cache: u8,
}

impl RangeEncoder<'_> {
    fn init(&mut self) {
        self.low = 0;
        self.range = 0xFFFF_FFFF;
        self.cache_size = 1;
        self.cache = 0;
    }

    fn flush_data(&mut self) {
        for _ in 0..5 {
            self.shift_low();
        }
    }

    fn shift_low(&mut self) {
        if (self.low as u32) < 0xFF00_0000 || (self.low >> 32) as u32 == 1 {
            let mut temp = self.cache;
            loop {
                self.out.push(temp.wrapping_add((self.low >> 32) as u8));
                temp = 0xFF;
                self.cache_size -= 1;
                if self.cache_size == 0 {
                    break;
                }
            }
            self.cache = ((self.low as u32) >> 24) as u8;
        }
        self.cache_size += 1;
        self.low = ((self.low as u32) << 8) as u64;
    }

    fn encode_direct_bits(&mut self, v: u32, num_total_bits: u32) {
        for i in (0..num_total_bits).rev() {
            self.range >>= 1;
            if (v >> i) & 1 == 1 {
                self.low += self.range as u64;
            }
            if self.range < TOP_VALUE {
                self.range <<= 8;
                self.shift_low();
            }
        }
    }

    /// `BitEncoder.Encode`.
    fn encode_bit(&mut self, prob: &mut u32, symbol: u32) {
        let new_bound = (self.range >> BIT_MODEL_TOTAL_BITS) * *prob;
        if symbol == 0 {
            self.range = new_bound;
            *prob += (BIT_MODEL_TOTAL - *prob) >> NUM_MOVE_BITS;
        } else {
            self.low += new_bound as u64;
            self.range -= new_bound;
            *prob -= *prob >> NUM_MOVE_BITS;
        }
        if self.range < TOP_VALUE {
            self.range <<= 8;
            self.shift_low();
        }
    }
}

const PROB_INIT: u32 = BIT_MODEL_TOTAL >> 1;

fn price(prob: u32, symbol: u32) -> u32 {
    PROB_PRICES[(((prob.wrapping_sub(symbol)) ^ 0u32.wrapping_sub(symbol)) & (BIT_MODEL_TOTAL - 1)) as usize
        >> NUM_MOVE_REDUCING_BITS]
}

fn price0(prob: u32) -> u32 {
    PROB_PRICES[(prob >> NUM_MOVE_REDUCING_BITS) as usize]
}

fn price1(prob: u32) -> u32 {
    PROB_PRICES[((BIT_MODEL_TOTAL - prob) >> NUM_MOVE_REDUCING_BITS) as usize]
}

/// `BitTreeEncoder`: `1 << bits` models, index 0 unused.
#[derive(Clone)]
struct BitTree {
    models: Vec<u32>,
    bits: u32,
}

impl BitTree {
    fn new(bits: u32) -> Self {
        Self { models: vec![PROB_INIT; 1 << bits], bits }
    }

    fn encode(&mut self, rc: &mut RangeEncoder<'_>, symbol: u32) {
        let mut m = 1usize;
        for bit_index in (0..self.bits).rev() {
            let bit = (symbol >> bit_index) & 1;
            rc.encode_bit(&mut self.models[m], bit);
            m = (m << 1) | bit as usize;
        }
    }

    fn reverse_encode(&mut self, rc: &mut RangeEncoder<'_>, symbol: u32) {
        reverse_encode(&mut self.models, 0, rc, self.bits, symbol);
    }

    fn price(&self, symbol: u32) -> u32 {
        let mut total = 0;
        let mut m = 1usize;
        for bit_index in (0..self.bits).rev() {
            let bit = (symbol >> bit_index) & 1;
            total += price(self.models[m], bit);
            m = (m << 1) + bit as usize;
        }
        total
    }

    fn reverse_price(&self, symbol: u32) -> u32 {
        reverse_price(&self.models, 0, self.bits, symbol)
    }
}

fn reverse_price(models: &[u32], start: u32, bits: u32, mut symbol: u32) -> u32 {
    let mut total = 0;
    let mut m = 1u32;
    for _ in 0..bits {
        let bit = symbol & 1;
        symbol >>= 1;
        total += price(models[start.wrapping_add(m) as usize], bit);
        m = (m << 1) | bit;
    }
    total
}

fn reverse_encode(models: &mut [u32], start: u32, rc: &mut RangeEncoder<'_>, bits: u32, mut symbol: u32) {
    let mut m = 1u32;
    for _ in 0..bits {
        let bit = symbol & 1;
        rc.encode_bit(&mut models[start.wrapping_add(m) as usize], bit);
        m = (m << 1) | bit;
        symbol >>= 1;
    }
}

// ---------------------------------------------------------------- state

#[derive(Clone, Copy, Default)]
struct State(u32);

impl State {
    fn update_char(&mut self) {
        self.0 = if self.0 < 4 {
            0
        } else if self.0 < 10 {
            self.0 - 3
        } else {
            self.0 - 6
        };
    }

    fn update_match(&mut self) {
        self.0 = if self.0 < 7 { 7 } else { 10 };
    }

    fn update_rep(&mut self) {
        self.0 = if self.0 < 7 { 8 } else { 11 };
    }

    fn update_short_rep(&mut self) {
        self.0 = if self.0 < 7 { 9 } else { 11 };
    }

    fn is_char_state(self) -> bool {
        self.0 < 7
    }
}

// -------------------------------------------------------------- literals

const LITERAL_CODER_SIZE: usize = 0x300;

fn literal_encode(probs: &mut [u32], rc: &mut RangeEncoder<'_>, symbol: u8) {
    let mut context = 1usize;
    for i in (0..8).rev() {
        let bit = ((symbol >> i) & 1) as u32;
        rc.encode_bit(&mut probs[context], bit);
        context = (context << 1) | bit as usize;
    }
}

fn literal_encode_matched(probs: &mut [u32], rc: &mut RangeEncoder<'_>, match_byte: u8, symbol: u8) {
    let mut context = 1usize;
    let mut same = true;
    for i in (0..8).rev() {
        let bit = ((symbol >> i) & 1) as u32;
        let mut state = context;
        if same {
            let match_bit = ((match_byte >> i) & 1) as u32;
            state += ((1 + match_bit) << 8) as usize;
            same = match_bit == bit;
        }
        rc.encode_bit(&mut probs[state], bit);
        context = (context << 1) | bit as usize;
    }
}

fn literal_price(probs: &[u32], match_mode: bool, match_byte: u8, symbol: u8) -> u32 {
    let mut total = 0;
    let mut context = 1usize;
    let mut i = 7i32;
    if match_mode {
        while i >= 0 {
            let match_bit = ((match_byte >> i) & 1) as u32;
            let bit = ((symbol >> i) & 1) as u32;
            total += price(probs[(((1 + match_bit) << 8) as usize) + context], bit);
            context = (context << 1) | bit as usize;
            if match_bit != bit {
                i -= 1;
                break;
            }
            i -= 1;
        }
    }
    while i >= 0 {
        let bit = ((symbol >> i) & 1) as u32;
        total += price(probs[context], bit);
        context = (context << 1) | bit as usize;
        i -= 1;
    }
    total
}

// --------------------------------------------------------------- lengths

/// `LenPriceTableEncoder` (with its `LenEncoder` base).
struct LenEncoder {
    choice: u32,
    choice2: u32,
    low: Vec<BitTree>,
    mid: Vec<BitTree>,
    high: BitTree,
    prices: Vec<u32>,
    table_size: u32,
    counters: [u32; NUM_POS_STATES_ENCODING_MAX],
}

impl LenEncoder {
    fn new() -> Self {
        Self {
            choice: PROB_INIT,
            choice2: PROB_INIT,
            low: vec![BitTree::new(NUM_LOW_LEN_BITS); NUM_POS_STATES_ENCODING_MAX],
            mid: vec![BitTree::new(NUM_MID_LEN_BITS); NUM_POS_STATES_ENCODING_MAX],
            high: BitTree::new(NUM_HIGH_LEN_BITS),
            prices: vec![0; (NUM_LEN_SYMBOLS as usize) << NUM_POS_STATES_BITS_MAX],
            table_size: 0,
            counters: [0; NUM_POS_STATES_ENCODING_MAX],
        }
    }

    fn encode_base(&mut self, rc: &mut RangeEncoder<'_>, mut symbol: u32, pos_state: u32) {
        if symbol < NUM_LOW_LEN_SYMBOLS {
            rc.encode_bit(&mut self.choice, 0);
            self.low[pos_state as usize].encode(rc, symbol);
        } else {
            symbol -= NUM_LOW_LEN_SYMBOLS;
            rc.encode_bit(&mut self.choice, 1);
            if symbol < NUM_MID_LEN_SYMBOLS {
                rc.encode_bit(&mut self.choice2, 0);
                self.mid[pos_state as usize].encode(rc, symbol);
            } else {
                rc.encode_bit(&mut self.choice2, 1);
                self.high.encode(rc, symbol - NUM_MID_LEN_SYMBOLS);
            }
        }
    }

    fn set_prices(&mut self, pos_state: u32, num_symbols: u32, st: u32) {
        let a0 = price0(self.choice);
        let a1 = price1(self.choice);
        let b0 = a1 + price0(self.choice2);
        let b1 = a1 + price1(self.choice2);
        let mut i = 0;
        while i < NUM_LOW_LEN_SYMBOLS {
            if i >= num_symbols {
                return;
            }
            self.prices[(st + i) as usize] = a0 + self.low[pos_state as usize].price(i);
            i += 1;
        }
        while i < NUM_LOW_LEN_SYMBOLS + NUM_MID_LEN_SYMBOLS {
            if i >= num_symbols {
                return;
            }
            self.prices[(st + i) as usize] = b0 + self.mid[pos_state as usize].price(i - NUM_LOW_LEN_SYMBOLS);
            i += 1;
        }
        while i < num_symbols {
            self.prices[(st + i) as usize] = b1 + self.high.price(i - NUM_LOW_LEN_SYMBOLS - NUM_MID_LEN_SYMBOLS);
            i += 1;
        }
    }

    fn price(&self, symbol: u32, pos_state: u32) -> u32 {
        self.prices[(pos_state * NUM_LEN_SYMBOLS + symbol) as usize]
    }

    fn update_table(&mut self, pos_state: u32) {
        self.set_prices(pos_state, self.table_size, pos_state * NUM_LEN_SYMBOLS);
        self.counters[pos_state as usize] = self.table_size;
    }

    fn update_tables(&mut self, num_pos_states: u32) {
        for pos_state in 0..num_pos_states {
            self.update_table(pos_state);
        }
    }

    fn encode(&mut self, rc: &mut RangeEncoder<'_>, symbol: u32, pos_state: u32) {
        self.encode_base(rc, symbol, pos_state);
        self.counters[pos_state as usize] = self.counters[pos_state as usize].wrapping_sub(1);
        if self.counters[pos_state as usize] == 0 {
            self.update_table(pos_state);
        }
    }
}

// --------------------------------------------------------- match finder

const HASH2_SIZE: u32 = 1 << 10;
const HASH3_SIZE: u32 = 1 << 16;
const BT2_HASH_SIZE: u32 = 1 << 16;
const START_MAX_LEN: u32 = 1;
const HASH3_OFFSET: u32 = HASH2_SIZE;
const EMPTY_HASH_VALUE: u32 = 0;
const MAX_VAL_FOR_NORMALIZE: u32 = (1 << 31) - 1;

/// The SDK's `BinTree` with its `InWindow` base, reading from a slice
/// instead of a stream.
struct BinTree<'a> {
    // InWindow
    input: &'a [u8],
    input_pos: usize,
    buffer: Vec<u8>,
    pos_limit: u32,
    stream_end_was_reached: bool,
    pointer_to_last_safe_position: u32,
    buffer_offset: u32,
    block_size: u32,
    pos: u32,
    keep_size_before: u32,
    keep_size_after: u32,
    stream_pos: u32,
    // BinTree
    cyclic_buffer_pos: u32,
    cyclic_buffer_size: u32,
    match_max_len: u32,
    son: Vec<u32>,
    hash: Vec<u32>,
    cut_value: u32,
    hash_mask: u32,
    hash_array: bool,
    num_hash_direct_bytes: u32,
    min_match_check: u32,
    fix_hash_size: u32,
}

impl<'a> BinTree<'a> {
    /// `SetType` then `Create(dictionarySize, kNumOpts, numFastBytes,
    /// kMatchMaxLen + 1)`, as `Encoder.Create` calls them.
    fn new(options: &EncoderOptions, input: &'a [u8]) -> Self {
        let hash_array = options.match_finder == MatchFinder::Bt4;
        let (num_hash_direct_bytes, min_match_check, fix_hash_size) =
            if hash_array { (0, 4, HASH2_SIZE + HASH3_SIZE) } else { (2, 3, 0) };

        let history_size = options.dictionary_size;
        let keep_add_buffer_before = NUM_OPTS;
        let match_max_len = options.fast_bytes;
        let keep_add_buffer_after = MATCH_MAX_LEN + 1;

        let cut_value = 16 + (match_max_len >> 1);
        let window_reserv_size =
            (history_size + keep_add_buffer_before + match_max_len + keep_add_buffer_after) / 2 + 256;
        let keep_size_before = history_size + keep_add_buffer_before;
        let keep_size_after = match_max_len + keep_add_buffer_after;
        let block_size = keep_size_before + keep_size_after + window_reserv_size;

        let cyclic_buffer_size = history_size + 1;
        let mut hash_mask = 0;
        let mut hs = BT2_HASH_SIZE;
        if hash_array {
            hs = history_size - 1;
            hs |= hs >> 1;
            hs |= hs >> 2;
            hs |= hs >> 4;
            hs |= hs >> 8;
            hs >>= 1;
            hs |= 0xFFFF;
            if hs > 1 << 24 {
                hs >>= 1;
            }
            hash_mask = hs;
            hs += 1;
            hs += fix_hash_size;
        }

        let mut tree = Self {
            input,
            input_pos: 0,
            buffer: vec![0; block_size as usize],
            pos_limit: 0,
            stream_end_was_reached: false,
            pointer_to_last_safe_position: block_size - keep_size_after,
            buffer_offset: 0,
            block_size,
            pos: 0,
            keep_size_before,
            keep_size_after,
            stream_pos: 0,
            cyclic_buffer_pos: 0,
            cyclic_buffer_size,
            match_max_len,
            son: vec![0; cyclic_buffer_size as usize * 2],
            hash: vec![0; hs as usize],
            cut_value,
            hash_mask,
            hash_array,
            num_hash_direct_bytes,
            min_match_check,
            fix_hash_size,
        };
        tree.init();
        tree
    }

    // InWindow

    fn move_block(&mut self) {
        // One more byte, since move_pos moves by one.
        let offset = self.buffer_offset.wrapping_add(self.pos).wrapping_sub(self.keep_size_before).saturating_sub(1);
        let num_bytes = self.buffer_offset.wrapping_add(self.stream_pos).wrapping_sub(offset);
        self.buffer.copy_within(offset as usize..(offset + num_bytes) as usize, 0);
        self.buffer_offset = self.buffer_offset.wrapping_sub(offset);
    }

    fn read_block(&mut self) {
        if self.stream_end_was_reached {
            return;
        }
        loop {
            let size = 0u32.wrapping_sub(self.buffer_offset).wrapping_add(self.block_size).wrapping_sub(self.stream_pos)
                as i32;
            if size == 0 {
                return;
            }
            let at = self.buffer_offset.wrapping_add(self.stream_pos) as usize;
            let n = (size as usize).min(self.input.len() - self.input_pos);
            self.buffer[at..at + n].copy_from_slice(&self.input[self.input_pos..self.input_pos + n]);
            self.input_pos += n;
            if n == 0 {
                self.pos_limit = self.stream_pos;
                let pointer_to_position = self.buffer_offset.wrapping_add(self.pos_limit);
                if pointer_to_position > self.pointer_to_last_safe_position {
                    self.pos_limit = self.pointer_to_last_safe_position.wrapping_sub(self.buffer_offset);
                }
                self.stream_end_was_reached = true;
                return;
            }
            self.stream_pos = self.stream_pos.wrapping_add(n as u32);
            if self.stream_pos >= self.pos.wrapping_add(self.keep_size_after) {
                self.pos_limit = self.stream_pos.wrapping_sub(self.keep_size_after);
            }
        }
    }

    fn window_move_pos(&mut self) {
        self.pos = self.pos.wrapping_add(1);
        if self.pos > self.pos_limit {
            let pointer_to_position = self.buffer_offset.wrapping_add(self.pos);
            if pointer_to_position > self.pointer_to_last_safe_position {
                self.move_block();
            }
            self.read_block();
        }
    }

    fn index_byte(&self, index: i32) -> u8 {
        let at = self.buffer_offset.wrapping_add(self.pos) as i64 + index as i64;
        self.buffer[at as usize]
    }

    fn match_len(&self, index: i32, distance: u32, mut limit: u32) -> u32 {
        if self.stream_end_was_reached {
            let at = self.pos as i64 + index as i64;
            if at + limit as i64 > self.stream_pos as i64 {
                limit = self.stream_pos.wrapping_sub(at as u32);
            }
        }
        let distance = distance.wrapping_add(1);
        let pby = self.buffer_offset.wrapping_add(self.pos).wrapping_add(index as u32);
        let mut i = 0;
        while i < limit
            && self.buffer[pby.wrapping_add(i) as usize]
                == self.buffer[pby.wrapping_add(i).wrapping_sub(distance) as usize]
        {
            i += 1;
        }
        i
    }

    fn num_available_bytes(&self) -> u32 {
        self.stream_pos.wrapping_sub(self.pos)
    }

    fn reduce_offsets(&mut self, sub_value: i32) {
        let sub = sub_value as u32;
        self.buffer_offset = self.buffer_offset.wrapping_add(sub);
        self.pos_limit = self.pos_limit.wrapping_sub(sub);
        self.pos = self.pos.wrapping_sub(sub);
        self.stream_pos = self.stream_pos.wrapping_sub(sub);
    }

    // BinTree

    fn init(&mut self) {
        // InWindow.Init
        self.buffer_offset = 0;
        self.pos = 0;
        self.stream_pos = 0;
        self.stream_end_was_reached = false;
        self.read_block();

        self.hash.fill(EMPTY_HASH_VALUE);
        self.cyclic_buffer_pos = 0;
        self.reduce_offsets(-1);
    }

    fn move_pos(&mut self) {
        self.cyclic_buffer_pos += 1;
        if self.cyclic_buffer_pos >= self.cyclic_buffer_size {
            self.cyclic_buffer_pos = 0;
        }
        self.window_move_pos();
        if self.pos == MAX_VAL_FOR_NORMALIZE {
            self.normalize();
        }
    }

    fn get_matches(&mut self, distances: &mut [u32]) -> u32 {
        let len_limit;
        if self.pos.wrapping_add(self.match_max_len) <= self.stream_pos {
            len_limit = self.match_max_len;
        } else {
            len_limit = self.stream_pos.wrapping_sub(self.pos);
            if len_limit < self.min_match_check {
                self.move_pos();
                return 0;
            }
        }

        let mut offset = 0usize;
        let match_min_pos = self.pos.saturating_sub(self.cyclic_buffer_size);
        let cur = self.buffer_offset.wrapping_add(self.pos) as usize;
        let mut max_len = START_MAX_LEN;
        let hash_value;
        let (mut hash2_value, mut hash3_value) = (0, 0);
        let buf = &self.buffer;

        if self.hash_array {
            let mut temp = CRC_TABLE[buf[cur] as usize] ^ buf[cur + 1] as u32;
            hash2_value = temp & (HASH2_SIZE - 1);
            temp ^= (buf[cur + 2] as u32) << 8;
            hash3_value = temp & (HASH3_SIZE - 1);
            hash_value = (temp ^ (CRC_TABLE[buf[cur + 3] as usize] << 5)) & self.hash_mask;
        } else {
            hash_value = buf[cur] as u32 ^ ((buf[cur + 1] as u32) << 8);
        }

        let mut cur_match = self.hash[(self.fix_hash_size + hash_value) as usize];
        if self.hash_array {
            let mut cur_match2 = self.hash[hash2_value as usize];
            let cur_match3 = self.hash[(HASH3_OFFSET + hash3_value) as usize];
            self.hash[hash2_value as usize] = self.pos;
            self.hash[(HASH3_OFFSET + hash3_value) as usize] = self.pos;
            if cur_match2 > match_min_pos
                && self.buffer[self.buffer_offset.wrapping_add(cur_match2) as usize] == self.buffer[cur]
            {
                max_len = 2;
                distances[offset] = max_len;
                distances[offset + 1] = self.pos - cur_match2 - 1;
                offset += 2;
            }
            if cur_match3 > match_min_pos
                && self.buffer[self.buffer_offset.wrapping_add(cur_match3) as usize] == self.buffer[cur]
            {
                if cur_match3 == cur_match2 {
                    offset -= 2;
                }
                max_len = 3;
                distances[offset] = max_len;
                distances[offset + 1] = self.pos - cur_match3 - 1;
                offset += 2;
                cur_match2 = cur_match3;
            }
            if offset != 0 && cur_match2 == cur_match {
                offset -= 2;
                max_len = START_MAX_LEN;
            }
        }

        self.hash[(self.fix_hash_size + hash_value) as usize] = self.pos;

        let mut ptr0 = ((self.cyclic_buffer_pos << 1) + 1) as usize;
        let mut ptr1 = (self.cyclic_buffer_pos << 1) as usize;
        let (mut len0, mut len1) = (self.num_hash_direct_bytes, self.num_hash_direct_bytes);

        if self.num_hash_direct_bytes != 0
            && cur_match > match_min_pos
            && self.buffer[self.buffer_offset.wrapping_add(cur_match).wrapping_add(self.num_hash_direct_bytes) as usize]
                != self.buffer[cur + self.num_hash_direct_bytes as usize]
        {
            max_len = self.num_hash_direct_bytes;
            distances[offset] = max_len;
            distances[offset + 1] = self.pos - cur_match - 1;
            offset += 2;
        }

        let mut count = self.cut_value;
        loop {
            if cur_match <= match_min_pos || count == 0 {
                self.son[ptr0] = EMPTY_HASH_VALUE;
                self.son[ptr1] = EMPTY_HASH_VALUE;
                break;
            }
            count -= 1;
            let delta = self.pos - cur_match;
            let cyclic_pos = (if delta <= self.cyclic_buffer_pos {
                self.cyclic_buffer_pos - delta
            } else {
                self.cyclic_buffer_pos.wrapping_sub(delta).wrapping_add(self.cyclic_buffer_size)
            } << 1) as usize;

            let pby1 = self.buffer_offset.wrapping_add(cur_match) as usize;
            let mut len = len0.min(len1);
            if self.buffer[pby1 + len as usize] == self.buffer[cur + len as usize] {
                loop {
                    len += 1;
                    if len == len_limit || self.buffer[pby1 + len as usize] != self.buffer[cur + len as usize] {
                        break;
                    }
                }
                if max_len < len {
                    max_len = len;
                    distances[offset] = max_len;
                    distances[offset + 1] = delta - 1;
                    offset += 2;
                    if len == len_limit {
                        self.son[ptr1] = self.son[cyclic_pos];
                        self.son[ptr0] = self.son[cyclic_pos + 1];
                        break;
                    }
                }
            }
            if self.buffer[pby1 + len as usize] < self.buffer[cur + len as usize] {
                self.son[ptr1] = cur_match;
                ptr1 = cyclic_pos + 1;
                cur_match = self.son[ptr1];
                len1 = len;
            } else {
                self.son[ptr0] = cur_match;
                ptr0 = cyclic_pos;
                cur_match = self.son[ptr0];
                len0 = len;
            }
        }
        self.move_pos();
        offset as u32
    }

    fn skip(&mut self, mut num: u32) {
        loop {
            'step: {
                let len_limit;
                if self.pos.wrapping_add(self.match_max_len) <= self.stream_pos {
                    len_limit = self.match_max_len;
                } else {
                    len_limit = self.stream_pos.wrapping_sub(self.pos);
                    if len_limit < self.min_match_check {
                        self.move_pos();
                        break 'step;
                    }
                }

                let match_min_pos = self.pos.saturating_sub(self.cyclic_buffer_size);
                let cur = self.buffer_offset.wrapping_add(self.pos) as usize;
                let hash_value;
                if self.hash_array {
                    let buf = &self.buffer;
                    let mut temp = CRC_TABLE[buf[cur] as usize] ^ buf[cur + 1] as u32;
                    let hash2_value = temp & (HASH2_SIZE - 1);
                    temp ^= (buf[cur + 2] as u32) << 8;
                    let hash3_value = temp & (HASH3_SIZE - 1);
                    hash_value = (temp ^ (CRC_TABLE[buf[cur + 3] as usize] << 5)) & self.hash_mask;
                    self.hash[hash2_value as usize] = self.pos;
                    self.hash[(HASH3_OFFSET + hash3_value) as usize] = self.pos;
                } else {
                    hash_value = self.buffer[cur] as u32 ^ ((self.buffer[cur + 1] as u32) << 8);
                }

                let mut cur_match = self.hash[(self.fix_hash_size + hash_value) as usize];
                self.hash[(self.fix_hash_size + hash_value) as usize] = self.pos;

                let mut ptr0 = ((self.cyclic_buffer_pos << 1) + 1) as usize;
                let mut ptr1 = (self.cyclic_buffer_pos << 1) as usize;
                let (mut len0, mut len1) = (self.num_hash_direct_bytes, self.num_hash_direct_bytes);

                let mut count = self.cut_value;
                loop {
                    if cur_match <= match_min_pos || count == 0 {
                        self.son[ptr0] = EMPTY_HASH_VALUE;
                        self.son[ptr1] = EMPTY_HASH_VALUE;
                        break;
                    }
                    count -= 1;
                    let delta = self.pos - cur_match;
                    let cyclic_pos = (if delta <= self.cyclic_buffer_pos {
                        self.cyclic_buffer_pos - delta
                    } else {
                        self.cyclic_buffer_pos.wrapping_sub(delta).wrapping_add(self.cyclic_buffer_size)
                    } << 1) as usize;

                    let pby1 = self.buffer_offset.wrapping_add(cur_match) as usize;
                    let mut len = len0.min(len1);
                    if self.buffer[pby1 + len as usize] == self.buffer[cur + len as usize] {
                        loop {
                            len += 1;
                            if len == len_limit || self.buffer[pby1 + len as usize] != self.buffer[cur + len as usize] {
                                break;
                            }
                        }
                        if len == len_limit {
                            self.son[ptr1] = self.son[cyclic_pos];
                            self.son[ptr0] = self.son[cyclic_pos + 1];
                            break;
                        }
                    }
                    if self.buffer[pby1 + len as usize] < self.buffer[cur + len as usize] {
                        self.son[ptr1] = cur_match;
                        ptr1 = cyclic_pos + 1;
                        cur_match = self.son[ptr1];
                        len1 = len;
                    } else {
                        self.son[ptr0] = cur_match;
                        ptr0 = cyclic_pos;
                        cur_match = self.son[ptr0];
                        len0 = len;
                    }
                }
                self.move_pos();
            }
            num -= 1;
            if num == 0 {
                break;
            }
        }
    }

    fn normalize(&mut self) {
        let sub_value = self.pos - self.cyclic_buffer_size;
        let normalize = |items: &mut [u32]| {
            for item in items {
                *item = if *item <= sub_value { EMPTY_HASH_VALUE } else { *item - sub_value };
            }
        };
        normalize(&mut self.son[..self.cyclic_buffer_size as usize * 2]);
        normalize(&mut self.hash);
        self.reduce_offsets(sub_value as i32);
    }
}

// --------------------------------------------------------------- encoder

#[derive(Clone, Copy, Default)]
struct Optimal {
    state: State,
    prev1_is_char: bool,
    prev2: bool,
    pos_prev2: u32,
    back_prev2: u32,
    price: u32,
    pos_prev: u32,
    back_prev: u32,
    backs: [u32; 4],
}

impl Optimal {
    fn make_as_char(&mut self) {
        self.back_prev = 0xFFFF_FFFF;
        self.prev1_is_char = false;
    }

    fn make_as_short_rep(&mut self) {
        self.back_prev = 0;
        self.prev1_is_char = false;
    }

    fn is_short_rep(&self) -> bool {
        self.back_prev == 0
    }
}

struct Encoder<'o> {
    rc: RangeEncoder<'o>,
    state: State,
    previous_byte: u8,
    rep_distances: [u32; NUM_REP_DISTANCES as usize],

    optimum: Vec<Optimal>,
    is_match: Vec<u32>,
    is_rep: [u32; NUM_STATES],
    is_rep_g0: [u32; NUM_STATES],
    is_rep_g1: [u32; NUM_STATES],
    is_rep_g2: [u32; NUM_STATES],
    is_rep0_long: Vec<u32>,
    pos_slot_encoder: Vec<BitTree>,
    pos_encoders: Vec<u32>,
    pos_align_encoder: BitTree,
    len_encoder: LenEncoder,
    rep_match_len_encoder: LenEncoder,
    literals: Vec<u32>,

    match_distances: Vec<u32>,
    num_fast_bytes: u32,
    longest_match_length: u32,
    num_distance_pairs: u32,
    additional_offset: u32,
    optimum_end_index: u32,
    optimum_current_index: u32,
    longest_match_was_found: bool,

    pos_slot_prices: Vec<u32>,
    distances_prices: Vec<u32>,
    align_prices: [u32; ALIGN_TABLE_SIZE as usize],
    align_price_count: u32,
    match_price_count: u32,
    temp_prices: Vec<u32>,
    dist_table_size: u32,

    pos_state_bits: u32,
    pos_state_mask: u32,
    num_literal_pos_state_bits: u32,
    num_literal_context_bits: u32,
    write_end_mark: bool,

    reps: [u32; NUM_REP_DISTANCES as usize],
    rep_lens: [u32; NUM_REP_DISTANCES as usize],
}

impl<'o> Encoder<'o> {
    fn new(options: &EncoderOptions, out: &'o mut Vec<u8>) -> Self {
        let mut dic_log_size = 0;
        while dic_log_size < 30 && options.dictionary_size > 1u32 << dic_log_size {
            dic_log_size += 1;
        }
        let literal_states = 1usize << (options.literal_context_bits + options.literal_pos_bits);
        Self {
            rc: RangeEncoder { out, low: 0, range: 0xFFFF_FFFF, cache_size: 1, cache: 0 },
            state: State::default(),
            previous_byte: 0,
            rep_distances: [0; 4],
            optimum: vec![Optimal::default(); NUM_OPTS as usize],
            is_match: vec![PROB_INIT; NUM_STATES << NUM_POS_STATES_BITS_MAX],
            is_rep: [PROB_INIT; NUM_STATES],
            is_rep_g0: [PROB_INIT; NUM_STATES],
            is_rep_g1: [PROB_INIT; NUM_STATES],
            is_rep_g2: [PROB_INIT; NUM_STATES],
            is_rep0_long: vec![PROB_INIT; NUM_STATES << NUM_POS_STATES_BITS_MAX],
            pos_slot_encoder: vec![BitTree::new(NUM_POS_SLOT_BITS); NUM_LEN_TO_POS_STATES as usize],
            pos_encoders: vec![PROB_INIT; (NUM_FULL_DISTANCES - END_POS_MODEL_INDEX) as usize],
            pos_align_encoder: BitTree::new(NUM_ALIGN_BITS),
            len_encoder: LenEncoder::new(),
            rep_match_len_encoder: LenEncoder::new(),
            literals: vec![PROB_INIT; literal_states * LITERAL_CODER_SIZE],
            match_distances: vec![0; MATCH_MAX_LEN as usize * 2 + 2],
            num_fast_bytes: options.fast_bytes,
            longest_match_length: 0,
            num_distance_pairs: 0,
            additional_offset: 0,
            optimum_end_index: 0,
            optimum_current_index: 0,
            longest_match_was_found: false,
            pos_slot_prices: vec![0; 1 << (NUM_POS_SLOT_BITS + NUM_LEN_TO_POS_STATES_BITS)],
            distances_prices: vec![0; (NUM_FULL_DISTANCES << NUM_LEN_TO_POS_STATES_BITS) as usize],
            align_prices: [0; ALIGN_TABLE_SIZE as usize],
            align_price_count: 0,
            match_price_count: 0,
            temp_prices: vec![0; NUM_FULL_DISTANCES as usize],
            dist_table_size: dic_log_size * 2,
            pos_state_bits: options.pos_state_bits,
            pos_state_mask: (1 << options.pos_state_bits) - 1,
            num_literal_pos_state_bits: options.literal_pos_bits,
            num_literal_context_bits: options.literal_context_bits,
            write_end_mark: options.end_marker,
            reps: [0; 4],
            rep_lens: [0; 4],
        }
    }

    /// The offset of `LiteralEncoder.GetSubCoder(pos, prevByte)`'s models.
    fn literal_coder(&self, pos: u32, prev_byte: u8) -> usize {
        let pos_mask = (1u32 << self.num_literal_pos_state_bits) - 1;
        let index = ((pos & pos_mask) << self.num_literal_context_bits)
            + ((prev_byte as u32) >> (8 - self.num_literal_context_bits));
        index as usize * LITERAL_CODER_SIZE
    }

    fn literal_price_at(&self, pos: u32, prev_byte: u8, match_mode: bool, match_byte: u8, symbol: u8) -> u32 {
        let at = self.literal_coder(pos, prev_byte);
        literal_price(&self.literals[at..at + LITERAL_CODER_SIZE], match_mode, match_byte, symbol)
    }

    /// `SetStreams` and the rest of `Code`.
    fn code(&mut self, mf: &mut BinTree<'_>) {
        self.rc.init();
        self.fill_distances_prices();
        self.fill_align_prices();
        let table_size = self.num_fast_bytes + 1 - MATCH_MIN_LEN;
        self.len_encoder.table_size = table_size;
        self.len_encoder.update_tables(1 << self.pos_state_bits);
        self.rep_match_len_encoder.table_size = table_size;
        self.rep_match_len_encoder.update_tables(1 << self.pos_state_bits);

        let mut now_pos: u64 = 0;
        if mf.num_available_bytes() == 0 {
            self.flush(now_pos as u32);
            return;
        }
        self.read_match_distances(mf);
        let pos_state = (now_pos as u32) & self.pos_state_mask;
        let complex = ((self.state.0 << NUM_POS_STATES_BITS_MAX) + pos_state) as usize;
        self.rc.encode_bit(&mut self.is_match[complex], 0);
        self.state.update_char();
        let cur_byte = mf.index_byte(0u32.wrapping_sub(self.additional_offset) as i32);
        let at = self.literal_coder(now_pos as u32, self.previous_byte);
        literal_encode(&mut self.literals[at..at + LITERAL_CODER_SIZE], &mut self.rc, cur_byte);
        self.previous_byte = cur_byte;
        self.additional_offset -= 1;
        now_pos += 1;

        if mf.num_available_bytes() == 0 {
            self.flush(now_pos as u32);
            return;
        }
        loop {
            let (len, mut pos) = self.get_optimum(mf, now_pos as u32);

            let pos_state = (now_pos as u32) & self.pos_state_mask;
            let complex = ((self.state.0 << NUM_POS_STATES_BITS_MAX) + pos_state) as usize;
            if len == 1 && pos == 0xFFFF_FFFF {
                self.rc.encode_bit(&mut self.is_match[complex], 0);
                let cur_byte = mf.index_byte(0u32.wrapping_sub(self.additional_offset) as i32);
                let at = self.literal_coder(now_pos as u32, self.previous_byte);
                let probs = &mut self.literals[at..at + LITERAL_CODER_SIZE];
                if !self.state.is_char_state() {
                    let match_byte = mf.index_byte(
                        0u32.wrapping_sub(self.rep_distances[0]).wrapping_sub(1).wrapping_sub(self.additional_offset)
                            as i32,
                    );
                    literal_encode_matched(probs, &mut self.rc, match_byte, cur_byte);
                } else {
                    literal_encode(probs, &mut self.rc, cur_byte);
                }
                self.previous_byte = cur_byte;
                self.state.update_char();
            } else {
                self.rc.encode_bit(&mut self.is_match[complex], 1);
                let s = self.state.0 as usize;
                if pos < NUM_REP_DISTANCES {
                    self.rc.encode_bit(&mut self.is_rep[s], 1);
                    if pos == 0 {
                        self.rc.encode_bit(&mut self.is_rep_g0[s], 0);
                        self.rc.encode_bit(&mut self.is_rep0_long[complex], if len == 1 { 0 } else { 1 });
                    } else {
                        self.rc.encode_bit(&mut self.is_rep_g0[s], 1);
                        if pos == 1 {
                            self.rc.encode_bit(&mut self.is_rep_g1[s], 0);
                        } else {
                            self.rc.encode_bit(&mut self.is_rep_g1[s], 1);
                            self.rc.encode_bit(&mut self.is_rep_g2[s], pos - 2);
                        }
                    }
                    if len == 1 {
                        self.state.update_short_rep();
                    } else {
                        self.rep_match_len_encoder.encode(&mut self.rc, len - MATCH_MIN_LEN, pos_state);
                        self.state.update_rep();
                    }
                    let distance = self.rep_distances[pos as usize];
                    if pos != 0 {
                        for i in (1..=pos as usize).rev() {
                            self.rep_distances[i] = self.rep_distances[i - 1];
                        }
                        self.rep_distances[0] = distance;
                    }
                } else {
                    self.rc.encode_bit(&mut self.is_rep[s], 0);
                    self.state.update_match();
                    self.len_encoder.encode(&mut self.rc, len - MATCH_MIN_LEN, pos_state);
                    pos -= NUM_REP_DISTANCES;
                    let pos_slot = pos_slot(pos);
                    let len_to_pos_state = len_to_pos_state(len);
                    self.pos_slot_encoder[len_to_pos_state as usize].encode(&mut self.rc, pos_slot);

                    if pos_slot >= START_POS_MODEL_INDEX {
                        let footer_bits = (pos_slot >> 1) - 1;
                        let base_val = (2 | (pos_slot & 1)) << footer_bits;
                        let pos_reduced = pos - base_val;
                        if pos_slot < END_POS_MODEL_INDEX {
                            reverse_encode(
                                &mut self.pos_encoders,
                                base_val.wrapping_sub(pos_slot + 1),
                                &mut self.rc,
                                footer_bits,
                                pos_reduced,
                            );
                        } else {
                            self.rc.encode_direct_bits(pos_reduced >> NUM_ALIGN_BITS, footer_bits - NUM_ALIGN_BITS);
                            self.pos_align_encoder.reverse_encode(&mut self.rc, pos_reduced & ALIGN_MASK);
                            self.align_price_count += 1;
                        }
                    }
                    for i in (1..NUM_REP_DISTANCES as usize).rev() {
                        self.rep_distances[i] = self.rep_distances[i - 1];
                    }
                    self.rep_distances[0] = pos;
                    self.match_price_count += 1;
                }
                self.previous_byte = mf.index_byte(len.wrapping_sub(1).wrapping_sub(self.additional_offset) as i32);
            }
            self.additional_offset -= len;
            now_pos += len as u64;
            if self.additional_offset == 0 {
                if self.match_price_count >= 1 << 7 {
                    self.fill_distances_prices();
                }
                if self.align_price_count >= ALIGN_TABLE_SIZE {
                    self.fill_align_prices();
                }
                if mf.num_available_bytes() == 0 {
                    self.flush(now_pos as u32);
                    return;
                }
            }
        }
    }

    fn flush(&mut self, now_pos: u32) {
        self.write_end_marker(now_pos & self.pos_state_mask);
        self.rc.flush_data();
    }

    fn write_end_marker(&mut self, pos_state: u32) {
        if !self.write_end_mark {
            return;
        }
        let complex = ((self.state.0 << NUM_POS_STATES_BITS_MAX) + pos_state) as usize;
        self.rc.encode_bit(&mut self.is_match[complex], 1);
        self.rc.encode_bit(&mut self.is_rep[self.state.0 as usize], 0);
        self.state.update_match();
        let len = MATCH_MIN_LEN;
        self.len_encoder.encode(&mut self.rc, len - MATCH_MIN_LEN, pos_state);
        let pos_slot = (1 << NUM_POS_SLOT_BITS) - 1;
        let len_to_pos_state = len_to_pos_state(len);
        self.pos_slot_encoder[len_to_pos_state as usize].encode(&mut self.rc, pos_slot);
        let footer_bits = 30;
        let pos_reduced = (1u32 << footer_bits) - 1;
        self.rc.encode_direct_bits(pos_reduced >> NUM_ALIGN_BITS, footer_bits - NUM_ALIGN_BITS);
        self.pos_align_encoder.reverse_encode(&mut self.rc, pos_reduced & ALIGN_MASK);
    }

    /// `ReadMatchDistances`: the longest match length and the number of
    /// distance pairs.
    fn read_match_distances(&mut self, mf: &mut BinTree<'_>) -> (u32, u32) {
        let mut len_res = 0;
        let num_distance_pairs = mf.get_matches(&mut self.match_distances);
        if num_distance_pairs > 0 {
            len_res = self.match_distances[num_distance_pairs as usize - 2];
            if len_res == self.num_fast_bytes {
                len_res += mf.match_len(
                    len_res as i32 - 1,
                    self.match_distances[num_distance_pairs as usize - 1],
                    MATCH_MAX_LEN - len_res,
                );
            }
        }
        self.additional_offset += 1;
        (len_res, num_distance_pairs)
    }

    fn move_pos(&mut self, mf: &mut BinTree<'_>, num: u32) {
        if num > 0 {
            mf.skip(num);
            self.additional_offset += num;
        }
    }

    fn rep_len1_price(&self, state: State, pos_state: u32) -> u32 {
        price0(self.is_rep_g0[state.0 as usize])
            + price0(self.is_rep0_long[((state.0 << NUM_POS_STATES_BITS_MAX) + pos_state) as usize])
    }

    fn pure_rep_price(&self, rep_index: u32, state: State, pos_state: u32) -> u32 {
        let s = state.0 as usize;
        if rep_index == 0 {
            price0(self.is_rep_g0[s])
                + price1(self.is_rep0_long[((state.0 << NUM_POS_STATES_BITS_MAX) + pos_state) as usize])
        } else {
            let mut total = price1(self.is_rep_g0[s]);
            if rep_index == 1 {
                total += price0(self.is_rep_g1[s]);
            } else {
                total += price1(self.is_rep_g1[s]);
                total += price(self.is_rep_g2[s], rep_index - 2);
            }
            total
        }
    }

    fn rep_price(&self, rep_index: u32, len: u32, state: State, pos_state: u32) -> u32 {
        self.rep_match_len_encoder.price(len - MATCH_MIN_LEN, pos_state)
            + self.pure_rep_price(rep_index, state, pos_state)
    }

    fn pos_len_price(&self, pos: u32, len: u32, pos_state: u32) -> u32 {
        let len_to_pos_state = len_to_pos_state(len);
        let total = if pos < NUM_FULL_DISTANCES {
            self.distances_prices[(len_to_pos_state * NUM_FULL_DISTANCES + pos) as usize]
        } else {
            self.pos_slot_prices[((len_to_pos_state << NUM_POS_SLOT_BITS) + pos_slot2(pos)) as usize]
                + self.align_prices[(pos & ALIGN_MASK) as usize]
        };
        total + self.len_encoder.price(len - MATCH_MIN_LEN, pos_state)
    }

    /// `Backward`: the length and back of the first step.
    fn backward(&mut self, mut cur: u32) -> (u32, u32) {
        self.optimum_end_index = cur;
        let mut pos_mem = self.optimum[cur as usize].pos_prev;
        let mut back_mem = self.optimum[cur as usize].back_prev;
        loop {
            if self.optimum[cur as usize].prev1_is_char {
                let o = self.optimum[cur as usize];
                self.optimum[pos_mem as usize].make_as_char();
                self.optimum[pos_mem as usize].pos_prev = pos_mem.wrapping_sub(1);
                if o.prev2 {
                    let p = &mut self.optimum[pos_mem as usize - 1];
                    p.prev1_is_char = false;
                    p.pos_prev = o.pos_prev2;
                    p.back_prev = o.back_prev2;
                }
            }
            let pos_prev = pos_mem;
            let back_cur = back_mem;

            back_mem = self.optimum[pos_prev as usize].back_prev;
            pos_mem = self.optimum[pos_prev as usize].pos_prev;

            self.optimum[pos_prev as usize].back_prev = back_cur;
            self.optimum[pos_prev as usize].pos_prev = cur;
            cur = pos_prev;
            if cur == 0 {
                break;
            }
        }
        let back_res = self.optimum[0].back_prev;
        self.optimum_current_index = self.optimum[0].pos_prev;
        (self.optimum_current_index, back_res)
    }

    /// `GetOptimum`: the length and back (`0xFFFFFFFF` for a literal, a rep
    /// index, or a distance plus 4) of the next step.
    fn get_optimum(&mut self, mf: &mut BinTree<'_>, mut position: u32) -> (u32, u32) {
        if self.optimum_end_index != self.optimum_current_index {
            let cur = self.optimum[self.optimum_current_index as usize];
            let len_res = cur.pos_prev - self.optimum_current_index;
            self.optimum_current_index = cur.pos_prev;
            return (len_res, cur.back_prev);
        }
        self.optimum_current_index = 0;
        self.optimum_end_index = 0;

        let (len_main, mut num_distance_pairs) = if !self.longest_match_was_found {
            self.read_match_distances(mf)
        } else {
            self.longest_match_was_found = false;
            (self.longest_match_length, self.num_distance_pairs)
        };

        // (The SDK also caps this at kMatchMaxLen, but never reads it again.)
        if mf.num_available_bytes() + 1 < 2 {
            return (1, 0xFFFF_FFFF);
        }

        let mut rep_max_index = 0;
        for i in 0..NUM_REP_DISTANCES as usize {
            self.reps[i] = self.rep_distances[i];
            self.rep_lens[i] = mf.match_len(-1, self.reps[i], MATCH_MAX_LEN);
            if self.rep_lens[i] > self.rep_lens[rep_max_index] {
                rep_max_index = i;
            }
        }
        if self.rep_lens[rep_max_index] >= self.num_fast_bytes {
            let len_res = self.rep_lens[rep_max_index];
            self.move_pos(mf, len_res - 1);
            return (len_res, rep_max_index as u32);
        }

        if len_main >= self.num_fast_bytes {
            let back_res = self.match_distances[num_distance_pairs as usize - 1] + NUM_REP_DISTANCES;
            self.move_pos(mf, len_main - 1);
            return (len_main, back_res);
        }

        let mut current_byte = mf.index_byte(-1);
        let mut match_byte = mf.index_byte(0u32.wrapping_sub(self.rep_distances[0]).wrapping_sub(2) as i32);

        if len_main < 2 && current_byte != match_byte && self.rep_lens[rep_max_index] < 2 {
            return (1, 0xFFFF_FFFF);
        }

        self.optimum[0].state = self.state;

        let mut pos_state = position & self.pos_state_mask;

        self.optimum[1].price = price0(self.is_match[((self.state.0 << NUM_POS_STATES_BITS_MAX) + pos_state) as usize])
            + self.literal_price_at(
                position,
                self.previous_byte,
                !self.state.is_char_state(),
                match_byte,
                current_byte,
            );
        self.optimum[1].make_as_char();

        let mut match_price = price1(self.is_match[((self.state.0 << NUM_POS_STATES_BITS_MAX) + pos_state) as usize]);
        let mut rep_match_price = match_price + price1(self.is_rep[self.state.0 as usize]);

        if match_byte == current_byte {
            let short_rep_price = rep_match_price + self.rep_len1_price(self.state, pos_state);
            if short_rep_price < self.optimum[1].price {
                self.optimum[1].price = short_rep_price;
                self.optimum[1].make_as_short_rep();
            }
        }

        let mut len_end =
            if len_main >= self.rep_lens[rep_max_index] { len_main } else { self.rep_lens[rep_max_index] };

        if len_end < 2 {
            return (1, self.optimum[1].back_prev);
        }

        self.optimum[1].pos_prev = 0;
        self.optimum[0].backs = self.reps;

        let mut len = len_end;
        loop {
            self.optimum[len as usize].price = INFINITY_PRICE;
            len -= 1;
            if len < 2 {
                break;
            }
        }

        for i in 0..NUM_REP_DISTANCES {
            let mut rep_len = self.rep_lens[i as usize];
            if rep_len < 2 {
                continue;
            }
            let price = rep_match_price + self.pure_rep_price(i, self.state, pos_state);
            loop {
                let cur_and_len_price = price + self.rep_match_len_encoder.price(rep_len - 2, pos_state);
                let optimum = &mut self.optimum[rep_len as usize];
                if cur_and_len_price < optimum.price {
                    optimum.price = cur_and_len_price;
                    optimum.pos_prev = 0;
                    optimum.back_prev = i;
                    optimum.prev1_is_char = false;
                }
                rep_len -= 1;
                if rep_len < 2 {
                    break;
                }
            }
        }

        let mut normal_match_price = match_price + price0(self.is_rep[self.state.0 as usize]);

        len = if self.rep_lens[0] >= 2 { self.rep_lens[0] + 1 } else { 2 };
        if len <= len_main {
            let mut offs = 0usize;
            while len > self.match_distances[offs] {
                offs += 2;
            }
            loop {
                let distance = self.match_distances[offs + 1];
                let cur_and_len_price = normal_match_price + self.pos_len_price(distance, len, pos_state);
                let optimum = &mut self.optimum[len as usize];
                if cur_and_len_price < optimum.price {
                    optimum.price = cur_and_len_price;
                    optimum.pos_prev = 0;
                    optimum.back_prev = distance + NUM_REP_DISTANCES;
                    optimum.prev1_is_char = false;
                }
                if len == self.match_distances[offs] {
                    offs += 2;
                    if offs as u32 == num_distance_pairs {
                        break;
                    }
                }
                len += 1;
            }
        }

        let mut cur = 0u32;

        loop {
            cur += 1;
            if cur == len_end {
                return self.backward(cur);
            }
            let (mut new_len, pairs) = self.read_match_distances(mf);
            num_distance_pairs = pairs;
            if new_len >= self.num_fast_bytes {
                self.num_distance_pairs = num_distance_pairs;
                self.longest_match_length = new_len;
                self.longest_match_was_found = true;
                return self.backward(cur);
            }
            position = position.wrapping_add(1);
            let cur_opt = self.optimum[cur as usize];
            let mut pos_prev = cur_opt.pos_prev;
            let mut state;
            if cur_opt.prev1_is_char {
                pos_prev -= 1;
                if cur_opt.prev2 {
                    state = self.optimum[cur_opt.pos_prev2 as usize].state;
                    if cur_opt.back_prev2 < NUM_REP_DISTANCES {
                        state.update_rep();
                    } else {
                        state.update_match();
                    }
                } else {
                    state = self.optimum[pos_prev as usize].state;
                }
                state.update_char();
            } else {
                state = self.optimum[pos_prev as usize].state;
            }
            if pos_prev == cur - 1 {
                if cur_opt.is_short_rep() {
                    state.update_short_rep();
                } else {
                    state.update_char();
                }
            } else {
                let pos;
                if cur_opt.prev1_is_char && cur_opt.prev2 {
                    pos_prev = cur_opt.pos_prev2;
                    pos = cur_opt.back_prev2;
                    state.update_rep();
                } else {
                    pos = cur_opt.back_prev;
                    if pos < NUM_REP_DISTANCES {
                        state.update_rep();
                    } else {
                        state.update_match();
                    }
                }
                let opt = self.optimum[pos_prev as usize];
                let b = opt.backs;
                self.reps = match pos {
                    0 => [b[0], b[1], b[2], b[3]],
                    1 => [b[1], b[0], b[2], b[3]],
                    2 => [b[2], b[0], b[1], b[3]],
                    3 => [b[3], b[0], b[1], b[2]],
                    _ => [pos - NUM_REP_DISTANCES, b[0], b[1], b[2]],
                };
            }
            self.optimum[cur as usize].state = state;
            self.optimum[cur as usize].backs = self.reps;
            let cur_price = self.optimum[cur as usize].price;

            current_byte = mf.index_byte(-1);
            match_byte = mf.index_byte(0u32.wrapping_sub(self.reps[0]).wrapping_sub(2) as i32);

            pos_state = position & self.pos_state_mask;

            let cur_and1_price = cur_price
                + price0(self.is_match[((state.0 << NUM_POS_STATES_BITS_MAX) + pos_state) as usize])
                + self.literal_price_at(position, mf.index_byte(-2), !state.is_char_state(), match_byte, current_byte);

            let next = (cur + 1) as usize;
            let mut next_is_char = false;
            if cur_and1_price < self.optimum[next].price {
                let o = &mut self.optimum[next];
                o.price = cur_and1_price;
                o.pos_prev = cur;
                o.make_as_char();
                next_is_char = true;
            }

            match_price =
                cur_price + price1(self.is_match[((state.0 << NUM_POS_STATES_BITS_MAX) + pos_state) as usize]);
            rep_match_price = match_price + price1(self.is_rep[state.0 as usize]);

            if match_byte == current_byte && !(self.optimum[next].pos_prev < cur && self.optimum[next].back_prev == 0) {
                let short_rep_price = rep_match_price + self.rep_len1_price(state, pos_state);
                if short_rep_price <= self.optimum[next].price {
                    let o = &mut self.optimum[next];
                    o.price = short_rep_price;
                    o.pos_prev = cur;
                    o.make_as_short_rep();
                    next_is_char = true;
                }
            }

            let num_available_bytes_full = (mf.num_available_bytes() + 1).min(NUM_OPTS - 1 - cur);
            let mut num_available_bytes = num_available_bytes_full;

            if num_available_bytes < 2 {
                continue;
            }
            if num_available_bytes > self.num_fast_bytes {
                num_available_bytes = self.num_fast_bytes;
            }
            if !next_is_char && match_byte != current_byte {
                // Try a literal and then rep0.
                let t = (num_available_bytes_full - 1).min(self.num_fast_bytes);
                let len_test2 = mf.match_len(0, self.reps[0], t);
                if len_test2 >= 2 {
                    let mut state2 = state;
                    state2.update_char();
                    let pos_state_next = position.wrapping_add(1) & self.pos_state_mask;
                    let next_rep_match_price = cur_and1_price
                        + price1(self.is_match[((state2.0 << NUM_POS_STATES_BITS_MAX) + pos_state_next) as usize])
                        + price1(self.is_rep[state2.0 as usize]);
                    let offset = cur + 1 + len_test2;
                    while len_end < offset {
                        len_end += 1;
                        self.optimum[len_end as usize].price = INFINITY_PRICE;
                    }
                    let cur_and_len_price = next_rep_match_price + self.rep_price(0, len_test2, state2, pos_state_next);
                    let optimum = &mut self.optimum[offset as usize];
                    if cur_and_len_price < optimum.price {
                        optimum.price = cur_and_len_price;
                        optimum.pos_prev = cur + 1;
                        optimum.back_prev = 0;
                        optimum.prev1_is_char = true;
                        optimum.prev2 = false;
                    }
                }
            }

            let mut start_len = 2;

            for rep_index in 0..NUM_REP_DISTANCES {
                let mut len_test = mf.match_len(-1, self.reps[rep_index as usize], num_available_bytes);
                if len_test < 2 {
                    continue;
                }
                let len_test_temp = len_test;
                loop {
                    while len_end < cur + len_test {
                        len_end += 1;
                        self.optimum[len_end as usize].price = INFINITY_PRICE;
                    }
                    let cur_and_len_price = rep_match_price + self.rep_price(rep_index, len_test, state, pos_state);
                    let optimum = &mut self.optimum[(cur + len_test) as usize];
                    if cur_and_len_price < optimum.price {
                        optimum.price = cur_and_len_price;
                        optimum.pos_prev = cur;
                        optimum.back_prev = rep_index;
                        optimum.prev1_is_char = false;
                    }
                    len_test -= 1;
                    if len_test < 2 {
                        break;
                    }
                }
                len_test = len_test_temp;

                if rep_index == 0 {
                    start_len = len_test + 1;
                }

                if len_test < num_available_bytes_full {
                    let t = (num_available_bytes_full - 1 - len_test).min(self.num_fast_bytes);
                    let len_test2 = mf.match_len(len_test as i32, self.reps[rep_index as usize], t);
                    if len_test2 >= 2 {
                        let mut state2 = state;
                        state2.update_rep();
                        let mut pos_state_next = position.wrapping_add(len_test) & self.pos_state_mask;
                        let cur_and_len_char_price = rep_match_price
                            + self.rep_price(rep_index, len_test, state, pos_state)
                            + price0(self.is_match[((state2.0 << NUM_POS_STATES_BITS_MAX) + pos_state_next) as usize])
                            + self.literal_price_at(
                                position.wrapping_add(len_test),
                                mf.index_byte(len_test as i32 - 2),
                                true,
                                mf.index_byte(len_test as i32 - 1 - (self.reps[rep_index as usize] + 1) as i32),
                                mf.index_byte(len_test as i32 - 1),
                            );
                        state2.update_char();
                        pos_state_next = position.wrapping_add(len_test + 1) & self.pos_state_mask;
                        let next_match_price = cur_and_len_char_price
                            + price1(self.is_match[((state2.0 << NUM_POS_STATES_BITS_MAX) + pos_state_next) as usize]);
                        let next_rep_match_price = next_match_price + price1(self.is_rep[state2.0 as usize]);

                        let offset = len_test + 1 + len_test2;
                        while len_end < cur + offset {
                            len_end += 1;
                            self.optimum[len_end as usize].price = INFINITY_PRICE;
                        }
                        let cur_and_len_price =
                            next_rep_match_price + self.rep_price(0, len_test2, state2, pos_state_next);
                        let optimum = &mut self.optimum[(cur + offset) as usize];
                        if cur_and_len_price < optimum.price {
                            optimum.price = cur_and_len_price;
                            optimum.pos_prev = cur + len_test + 1;
                            optimum.back_prev = 0;
                            optimum.prev1_is_char = true;
                            optimum.prev2 = true;
                            optimum.pos_prev2 = cur;
                            optimum.back_prev2 = rep_index;
                        }
                    }
                }
            }

            if new_len > num_available_bytes {
                new_len = num_available_bytes;
                num_distance_pairs = 0;
                while new_len > self.match_distances[num_distance_pairs as usize] {
                    num_distance_pairs += 2;
                }
                self.match_distances[num_distance_pairs as usize] = new_len;
                num_distance_pairs += 2;
            }
            if new_len >= start_len {
                normal_match_price = match_price + price0(self.is_rep[state.0 as usize]);
                while len_end < cur + new_len {
                    len_end += 1;
                    self.optimum[len_end as usize].price = INFINITY_PRICE;
                }

                let mut offs = 0usize;
                while start_len > self.match_distances[offs] {
                    offs += 2;
                }

                let mut len_test = start_len;
                loop {
                    let cur_back = self.match_distances[offs + 1];
                    let mut cur_and_len_price = normal_match_price + self.pos_len_price(cur_back, len_test, pos_state);
                    let optimum = &mut self.optimum[(cur + len_test) as usize];
                    if cur_and_len_price < optimum.price {
                        optimum.price = cur_and_len_price;
                        optimum.pos_prev = cur;
                        optimum.back_prev = cur_back + NUM_REP_DISTANCES;
                        optimum.prev1_is_char = false;
                    }

                    if len_test == self.match_distances[offs] {
                        if len_test < num_available_bytes_full {
                            let t = (num_available_bytes_full - 1 - len_test).min(self.num_fast_bytes);
                            let len_test2 = mf.match_len(len_test as i32, cur_back, t);
                            if len_test2 >= 2 {
                                let mut state2 = state;
                                state2.update_match();
                                let mut pos_state_next = position.wrapping_add(len_test) & self.pos_state_mask;
                                let cur_and_len_char_price = cur_and_len_price
                                    + price0(
                                        self.is_match
                                            [((state2.0 << NUM_POS_STATES_BITS_MAX) + pos_state_next) as usize],
                                    )
                                    + self.literal_price_at(
                                        position.wrapping_add(len_test),
                                        mf.index_byte(len_test as i32 - 2),
                                        true,
                                        mf.index_byte(len_test as i32 - (cur_back + 1) as i32 - 1),
                                        mf.index_byte(len_test as i32 - 1),
                                    );
                                state2.update_char();
                                pos_state_next = position.wrapping_add(len_test + 1) & self.pos_state_mask;
                                let next_match_price = cur_and_len_char_price
                                    + price1(
                                        self.is_match
                                            [((state2.0 << NUM_POS_STATES_BITS_MAX) + pos_state_next) as usize],
                                    );
                                let next_rep_match_price = next_match_price + price1(self.is_rep[state2.0 as usize]);

                                let offset = len_test + 1 + len_test2;
                                while len_end < cur + offset {
                                    len_end += 1;
                                    self.optimum[len_end as usize].price = INFINITY_PRICE;
                                }
                                cur_and_len_price =
                                    next_rep_match_price + self.rep_price(0, len_test2, state2, pos_state_next);
                                let optimum = &mut self.optimum[(cur + offset) as usize];
                                if cur_and_len_price < optimum.price {
                                    optimum.price = cur_and_len_price;
                                    optimum.pos_prev = cur + len_test + 1;
                                    optimum.back_prev = 0;
                                    optimum.prev1_is_char = true;
                                    optimum.prev2 = true;
                                    optimum.pos_prev2 = cur;
                                    optimum.back_prev2 = cur_back + NUM_REP_DISTANCES;
                                }
                            }
                        }
                        offs += 2;
                        if offs as u32 == num_distance_pairs {
                            break;
                        }
                    }
                    len_test += 1;
                }
            }
        }
    }

    fn fill_distances_prices(&mut self) {
        for i in START_POS_MODEL_INDEX..NUM_FULL_DISTANCES {
            let pos_slot = pos_slot(i);
            let footer_bits = (pos_slot >> 1) - 1;
            let base_val = (2 | (pos_slot & 1)) << footer_bits;
            self.temp_prices[i as usize] =
                reverse_price(&self.pos_encoders, base_val.wrapping_sub(pos_slot + 1), footer_bits, i - base_val);
        }

        for len_to_pos_state in 0..NUM_LEN_TO_POS_STATES {
            let encoder = &self.pos_slot_encoder[len_to_pos_state as usize];
            let st = len_to_pos_state << NUM_POS_SLOT_BITS;
            for pos_slot in 0..self.dist_table_size {
                self.pos_slot_prices[(st + pos_slot) as usize] = encoder.price(pos_slot);
            }
            for pos_slot in END_POS_MODEL_INDEX..self.dist_table_size {
                self.pos_slot_prices[(st + pos_slot) as usize] +=
                    (((pos_slot >> 1) - 1) - NUM_ALIGN_BITS) << NUM_BIT_PRICE_SHIFT_BITS;
            }

            let st2 = len_to_pos_state * NUM_FULL_DISTANCES;
            for i in 0..START_POS_MODEL_INDEX {
                self.distances_prices[(st2 + i) as usize] = self.pos_slot_prices[(st + i) as usize];
            }
            for i in START_POS_MODEL_INDEX..NUM_FULL_DISTANCES {
                self.distances_prices[(st2 + i) as usize] =
                    self.pos_slot_prices[(st + pos_slot(i)) as usize] + self.temp_prices[i as usize];
            }
        }
        self.match_price_count = 0;
    }

    fn fill_align_prices(&mut self) {
        for i in 0..ALIGN_TABLE_SIZE {
            self.align_prices[i as usize] = self.pos_align_encoder.reverse_price(i);
        }
        self.align_price_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(data: &[u8], options: &EncoderOptions) {
        let encoded = encode(data, options).unwrap();
        let mut decoded = vec![0u8; data.len()];
        super::super::lzma::decode(&encoded[..PROPERTIES_SIZE], &encoded[PROPERTIES_SIZE..], &mut decoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn round_trips() {
        let mut data = Vec::new();
        let mut x = 1u32;
        for i in 0..200_000u32 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            data.push(if i % 3 == 0 { (x >> 24) as u8 } else { b"steamkit"[(x >> 28) as usize % 8] });
        }
        for options in [
            EncoderOptions::VZIP,
            EncoderOptions { match_finder: MatchFinder::Bt2, fast_bytes: 32, ..EncoderOptions::VZIP },
            EncoderOptions {
                dictionary_size: 1 << 12,
                pos_state_bits: 0,
                literal_context_bits: 0,
                ..EncoderOptions::VZIP
            },
            EncoderOptions { literal_pos_bits: 2, literal_context_bits: 1, fast_bytes: 273, ..EncoderOptions::VZIP },
        ] {
            round_trip(&[], &options);
            round_trip(b"a", &options);
            round_trip(&data, &options);
            round_trip(&vec![7u8; 100_000], &options);
        }
    }

    #[test]
    fn rejects_bad_options() {
        assert!(encode(b"x", &EncoderOptions { fast_bytes: 4, ..EncoderOptions::VZIP }).is_none());
        assert!(encode(b"x", &EncoderOptions { pos_state_bits: 5, ..EncoderOptions::VZIP }).is_none());
    }
}
