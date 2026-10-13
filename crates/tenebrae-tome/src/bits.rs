//! Bit-level encodings shared by the tome's sections.

/// Number of 32-bit words holding `count` fields of `width` bits, packed LSB-first.
pub(crate) fn packed_words(count: usize, width: u32) -> usize {
    (count * width as usize).div_ceil(32)
}

/// Reads field `index` of `width` bits from LSB-first packed words (a field may straddle words).
pub(crate) fn field(words: &[u32], index: usize, width: u32) -> u32 {
    if width == 0 {
        return 0;
    }
    let bit = index * width as usize;
    let lo = u64::from(words.get(bit >> 5).copied().unwrap_or(0));
    let hi = u64::from(words.get((bit >> 5) + 1).copied().unwrap_or(0));
    let window = lo | (hi << 32);
    let mask = if width >= 32 {
        u64::from(u32::MAX)
    } else {
        (1u64 << width) - 1
    };
    ((window >> (bit & 31)) & mask) as u32
}

/// Packs `values` as `width`-bit fields LSB-first into `words` words (extra words stay zero).
pub(crate) fn pack(values: impl IntoIterator<Item = u32>, width: u32, words: usize) -> Vec<u32> {
    let mut out = vec![0u32; words];
    if width == 0 {
        return out;
    }
    for (index, value) in values.into_iter().enumerate() {
        let bit = index * width as usize;
        let shifted = u64::from(value) << (bit & 31);
        out[bit >> 5] |= shifted as u32;
        let high = (shifted >> 32) as u32;
        if high != 0 {
            out[(bit >> 5) + 1] |= high;
        }
    }
    out
}

/// A KD node code: 0, 1 and 2 split on x, y and z; 3 is a leaf.
pub(crate) const LEAF: u8 = 3;

/// The rank directory the engine stores after a KD tree's 2-bit node codes.
///
/// For `n` nodes it holds three levels of inner-node counts (nodes whose code is not 3):
/// a `u32` per 65,536 nodes (count from 0), then a `u16` per 256 nodes that do not start a
/// 65,536 block (count since the last 65,536 boundary), packed two per word, then a `u8` per
/// 16 nodes that do not start a 256 block (count since the last 256 boundary), packed four per
/// word. Each level starts on a word.
pub(crate) fn rank_directory(codes: &[u8]) -> Vec<u32> {
    let n = codes.len();
    let mut inner_before = Vec::with_capacity(n + 1);
    inner_before.push(0u32);
    for &code in codes {
        let last = *inner_before.last().unwrap_or(&0);
        inner_before.push(last + u32::from(code != LEAF));
    }
    let l1 = n >> 16;
    let l2 = (n >> 8) - l1;
    let l3 = (n >> 4) - (n >> 8);
    let mut words = vec![0u32; l1 + l2.div_ceil(2) + l3.div_ceil(4)];
    for block in 1..=l1 {
        words[block - 1] = inner_before[block << 16];
    }
    let l2_base = l1;
    for block in 1..=(n >> 8) {
        if block % 256 == 0 {
            continue;
        }
        let index = block - (block >> 8) - 1;
        let value = inner_before[block << 8] - inner_before[(block >> 8) << 16];
        words[l2_base + index / 2] |= (value & 0xFFFF) << (16 * (index & 1));
    }
    let l3_base = l2_base + l2.div_ceil(2);
    for block in 1..=(n >> 4) {
        if block % 16 == 0 {
            continue;
        }
        let index = block - (block >> 4) - 1;
        let value = inner_before[block << 4] - inner_before[(block >> 4) << 8];
        words[l3_base + index / 4] |= (value & 0xFF) << (8 * (index & 3));
    }
    words
}

/// Packs 2-bit KD node codes, 16 per word.
pub(crate) fn pack_codes(codes: &[u8]) -> Vec<u32> {
    pack(
        codes.iter().map(|&c| u32::from(c)),
        2,
        packed_words(codes.len(), 2),
    )
}

/// Number of words a KD tree of `n` nodes occupies: codes plus rank directory.
#[cfg(test)]
fn kd_words(n: usize) -> usize {
    let l1 = n >> 16;
    let l2 = (n >> 8) - l1;
    let l3 = (n >> 4) - (n >> 8);
    packed_words(n, 2) + l1 + l2.div_ceil(2) + l3.div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip_across_word_boundaries() {
        let values: Vec<u32> = (0..100).map(|i| (i * 2654435761u32) & 0x1FFF).collect();
        let words = pack(values.iter().copied(), 13, packed_words(values.len(), 13));
        for (i, v) in values.iter().enumerate() {
            assert_eq!(field(&words, i, 13), *v);
        }
    }

    #[test]
    fn rank_directory_sizes_match_a_stock_top_tree() {
        // A stock tome: 529 top nodes = 34 code words + 9 rank words (0x140 + 172 = 0x1EC).
        let codes = vec![LEAF; 529];
        assert_eq!(packed_words(529, 2), 34);
        assert_eq!(rank_directory(&codes).len(), 9);
        assert_eq!(kd_words(529), 43);
    }
}
