use core::codec::Codec;
use core::index::MergeState;
use core::store::Directory;

/// Computes which segments have identical field name to number mappings,
/// which allows stored fields and term vectors in this codec to be bulk-merged.
pub struct MatchingReaders {
    /// `SegmentReader`s that have identical field name/number mapping,
    /// so their stored fields and term vectors may be bulk merged.
    pub matching_readers: Vec<bool>,
    /// How many #matching_readers are set
    pub count: usize,
}

impl MatchingReaders {
    pub fn new<D: Directory, C: Codec>(merge_state: &MergeState<D, C>) -> Self {
        // If the i'th reader is a SegmentReader and has
        // identical fieldName -> number mapping, then this
        // array will be non-null at position i:
        let num_readers = merge_state.max_docs.len();
        let mut matched_count = 0;

        let mut matching_readers = vec![false; num_readers];

        // If this reader is a SegmentReader, and all of its
        // field name -> number mappings match the "merged"
        // FieldInfos, then we can do a bulk copy of the
        // stored fields:
        'next_reader: for i in 0..num_readers {
            for fi in merge_state.fields_infos[i].by_number.values() {
                let other = merge_state
                    .merge_field_infos
                    .as_ref()
                    .unwrap()
                    .field_info_by_number(fi.number);
                if other.map_or(true, |o| o.name != fi.name) {
                    continue 'next_reader;
                }
            }
            matching_readers[i] = true;
            matched_count += 1;
        }
        MatchingReaders {
            matching_readers,
            count: matched_count,
        }
    }
}
