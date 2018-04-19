use std::sync::{Arc, Mutex};

use core::search::{DocIterator, NO_MORE_DOCS};
use core::util::bit_util::{self, UnsignedShift};
use core::util::ImmutableBits;

use error::*;

pub type BitSetRef = Arc<Mutex<Box<BitSet>>>;
pub type ImmutableBitSetRef = Arc<ImmutableBitSet>;

pub trait ImmutableBitSet: ImmutableBits {
    /// Return the number of bits that are set.
    /// this method is likely to run in linear time
    fn cardinality(&self) -> usize;

    fn approximate_cardinality(&self) -> usize {
        self.cardinality()
    }

    /// Returns the index of the first set bit starting at the index specified.
    /// `DocIdSetIterator#NO_MORE_DOCS` is returned if there are no more set bits.
    fn next_set_bit(&self, index: usize) -> i32;

    fn assert_unpositioned(&self, iter: &DocIterator) -> Result<()> {
        if iter.doc_id() != -1 {
            bail!(ErrorKind::IllegalState(format!(
                "This operation only works with an unpositioned iterator, got current position = \
                 {}",
                iter.doc_id()
            )))
        }
        Ok(())
    }
}

/// Base implementation for a bit set.
pub trait BitSet: ImmutableBitSet {
    fn set(&mut self, i: usize);
    /// Clears a range of bits.
    fn clear(&mut self, start_index: usize, end_index: usize);

    /// Does in-place OR of the bits provided by the iterator. The state of the
    /// iterator after this operation terminates is undefined.
    fn or(&mut self, iter: &mut DocIterator) -> Result<()> {
        self.assert_unpositioned(iter)?;
        loop {
            let doc = iter.next()?;
            if doc == NO_MORE_DOCS {
                break;
            }
            self.set(doc as usize);
        }
        Ok(())
    }
}

/// BitSet of fixed length (numBits), backed by accessible `#getBits`
/// long[], accessed with an int index, implementing {@link Bits} and
/// `DocIdSet`. If you need to manage more than 2.1B bits, use
/// `LongBitSet`.
///
pub struct FixedBitSet {
    pub bits: Vec<i64>,
    // Array of longs holding the bits
    pub num_bits: usize,
    // The number of bits in use
    pub num_words: usize,
    // The exact number of longs needed to hold numBits (<= bits.length)
}

impl FixedBitSet {
    /// Creates a new LongBitSet.
    /// The internally allocated long array will be exactly the size needed to accommodate the
    /// numBits specified. @param numBits the number of bits needed
    ///
    pub fn new(num_bits: usize) -> FixedBitSet {
        let num_words = bits2words(num_bits);
        let bits = vec![0i64; num_words];
        FixedBitSet {
            num_bits,
            bits,
            num_words,
        }
    }

    /// Creates a new LongBitSet using the provided long[] array as backing store.
    /// The storedBits array must be large enough to accommodate the numBits specified, but may be
    /// larger. In that case the 'extra' or 'ghost' bits must be clear (or they may provoke
    /// spurious side-effects) @param storedBits the array to use as backing store
    /// @param numBits the number of bits actually needed
    ///
    pub fn copy_from(stored_bits: Vec<i64>, num_bits: usize) -> Result<FixedBitSet> {
        let num_words = bits2words(num_bits);
        if num_words > stored_bits.len() {
            bail!(ErrorKind::IllegalArgument(format!(
                "The given long array is too small  to hold {} bits.",
                num_bits
            )));
        }

        let bits = FixedBitSet {
            bits: stored_bits,
            num_words,
            num_bits,
        };
        assert!(bits.verify_ghost_bits_clear());
        Ok(bits)
    }

    /// If the given {@link FixedBitSet} is large enough to hold {@code numBits+1},
    /// returns the given bits, otherwise returns a new {@link FixedBitSet} which
    /// can hold the requested number of bits.
    ///
    /// NOTE: the returned bitset reuses the underlying {@code long[]} of
    /// the given `bits` if possible. Also, calling {@link #length()} on the
    /// returned bits may return a value greater than {@code numBits}.
    ///
    pub fn ensure_capacity(&mut self, num_bits: usize) {
        if num_bits >= self.num_bits {
            // Depends on the ghost bits being clear!
            // (Otherwise, they may become visible in the new instance)
            let num_words = bits2words(num_bits);
            if num_words >= self.bits.len() {
                self.bits.resize(num_words + 1usize, 0i64);
                self.num_words = num_words + 1usize;
                self.num_bits = self.num_words << 6usize;
            }
        }
    }

    /// Checks if the bits past numBits are clear. Some methods rely on this implicit
    /// assumption: search for "Depends on the ghost bits being clear!"
    /// @return true if the bits past numBits are clear.
    fn verify_ghost_bits_clear(&self) -> bool {
        for i in self.num_words..self.bits.len() {
            if self.bits[i] != 0 {
                return false;
            }
        }
        if self.num_bits.trailing_zeros() >= 6 {
            return true;
        }
        let mask = -1i64 << self.num_bits;
        (self.bits[self.num_words - 1] & mask) == 0
    }

    pub fn clear(&mut self, index: i32) {
        assert!(index < self.num_bits as i32);
        let word_num = index >> 6;
        let mask = 1i64 << i64::from(index & 0x3fi32);
        self.bits[word_num as usize] &= !mask;
    }

    pub fn flip(&mut self, start_index: usize, end_index: usize) {
        debug_assert!(start_index < self.num_bits);
        debug_assert!(end_index <= self.num_bits);
        if end_index <= start_index {
            return;
        }
        let start_word = start_index >> 6;
        let end_word = (end_index - 1) >> 6;

        let start_mask = !((-1i64) << (start_index & 0x3fusize)) as i64;
        let end_mask = !((-1i64).unsigned_shift((64usize - end_index) & 0x3fusize));

        if start_word == end_word {
            self.bits[start_word] ^= start_mask | end_mask;
            return;
        }

        // optimize tight loop with unsafe
        self.bits[start_word] ^= start_mask;
        unsafe {
            let ptr = self.bits.as_mut_ptr();
            for i in start_word + 1..end_word {
                let e = ptr.offset(i as isize);
                *e = !*e;
            }
        }
        self.bits[end_word] ^= end_mask;
    }
}

impl ImmutableBitSet for FixedBitSet {
    fn cardinality(&self) -> usize {
        bit_util::pop_array(&self.bits, 0, self.num_words)
    }

    fn next_set_bit(&self, index: usize) -> i32 {
        // Depends on the ghost bits being clear!
        debug_assert!(index < self.num_bits);
        let mut i = index >> 6;
        // skip all the bits to the right of index
        let word = unsafe { *self.bits.as_ptr().offset(i as isize) } >> (index & 0x3fusize);

        if word != 0 {
            return (index as u32 + word.trailing_zeros()) as i32;
        }

        unsafe {
            let bits_ptr = self.bits.as_ptr();
            loop {
                i += 1;
                if i >= self.num_words {
                    break;
                }
                let word = *bits_ptr.offset(i as isize);
                if word != 0 {
                    return ((i << 6) as u32 + word.trailing_zeros()) as i32;
                }
            }
        }
        NO_MORE_DOCS
    }
}

impl BitSet for FixedBitSet {
    #[inline]
    fn set(&mut self, index: usize) {
        debug_assert!(index < self.num_bits);
        let word_num = index >> 6;
        let mask = 1i64 << (index & 0x3fusize);
        unsafe {
            *self.bits.as_mut_ptr().offset(word_num as isize) |= mask;
        }
    }

    fn clear(&mut self, start_index: usize, end_index: usize) {
        debug_assert!(start_index < self.num_bits);
        debug_assert!(end_index <= self.num_bits);
        if end_index <= start_index {
            return;
        }
        let start_word = start_index >> 6;
        let end_word = (end_index - 1) >> 6;

        // invert mask since we ar clear
        let start_mask = !((-1i64) << (start_index & 0x3fusize)) as i64;
        let end_mask = !((-1i64).unsigned_shift((64usize - end_index) & 0x3fusize));

        if start_word == end_word {
            self.bits[start_word] &= start_mask | end_mask;
            return;
        }

        self.bits[start_word] &= start_mask;
        for i in start_word + 1..end_word {
            self.bits[i] = 0i64;
        }
        self.bits[end_word] &= end_mask;
    }
}

impl ImmutableBits for FixedBitSet {
    #[inline]
    fn get(&self, index: usize) -> Result<bool> {
        debug_assert!(index < self.num_bits);
        let i = index >> 6; // div 64
                            // signed shift will keep a negative index and force an
                            // array-index-out-of-bounds-exception, removing the need for an explicit check.
        let mask = 1i64 << (index & 0x3fusize);
        Ok(unsafe { *self.bits.as_ptr().offset(i as isize) & mask != 0 })
    }

    fn len(&self) -> usize {
        self.num_bits
    }
}

/// returns the number of 64 bit words it would take to hold numBits
pub fn bits2words(num_bits: usize) -> usize {
    let num_bits = num_bits as i32;
    // I.e.: get the word-offset of the last bit and add one (make sure to use >> so 0 returns 0!)
    (((num_bits - 1) >> 6) + 1) as usize
}