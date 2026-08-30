use std::{ops::Range, slice::Iter};

// Iterate over the bits of a byte, from least- to most-significant.
#[derive(Clone, Debug)]
struct BitsIter {
    byte: u8,
    shift: u8,
}

impl BitsIter {
    /// Construct a new [`BitsIter`] to loop over the bits in the supplied byte.
    fn new(byte: u8) -> Self {
        Self { byte, shift: 0 }
    }
}

impl Iterator for BitsIter {
    type Item = bool;

    fn next(&mut self) -> Option<Self::Item> {
        if self.shift >= 8 {
            None
        } else {
            let out = (self.byte & 1 << self.shift) != 0;
            self.shift += 1;

            Some(out)
        }
    }
}

/// Loop over PVS indices, given run-length encoded visibility data. The logic is easier to
/// understand as a coroutine, this is the behavior we expect to implement:
///
/// ```ignore
/// std::iter::from_coroutine(
/// #[coroutine]
/// || {
///     let mut vis_leaf = 1;
///     let mut it = vis_data[vis_list..].iter();
///
///     while vis_leaf < num_leaves {
///         let byte = it.next().unwrap();
///         match *byte {
///             // a zero byte signals the start of an RLE sequence
///             0 => visleaf += 8 * *it.next().unwrap() as usize,
///
///             bits => {
///                 for shift in 0..8 {
///                     if bits & 1 << shift != 0 {
///                         yield vis_leaf;
///                     }
///
///                         vis_leaf += 1;
///                     }
///                 }
///             }
///         }
///     },
/// )
/// ```
///
/// Above code adapted from [Richter](https://github.com/cormac-obrien/richter/blob/506504d5f9f93dab807e61ba3cad1a27d6d5a707/src/common/bsp/mod.rs#L831-L866),
/// itself adapted from [code by David Etherton and Tony Myles](https://www.gamers.org/dEngine/quake/spec/quake-spec34/qkspec_4.htm#BL4).
#[derive(Clone)]
pub struct VisdataIter<'a> {
    vis_clusters: Range<u32>,
    data_bytes: Iter<'a, u8>,
    /// If `Some`, we are currently iterating over the bits in a byte.
    cur_byte: Option<BitsIter>,
}

impl Iterator for VisdataIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(is_visible) = self.cur_byte.as_mut().and_then(|byte| byte.next()) {
                let value = self.vis_clusters.next();
                if is_visible {
                    break value;
                }
            } else if self.vis_clusters.is_empty() {
                break None;
            } else {
                debug_assert_eq!(self.vis_clusters.start % 8, 0);

                let next_byte = *self.data_bytes.next()?;
                match next_byte {
                    0 => {
                        let advance_by_bytes = *self.data_bytes.next()?;
                        let advance_by_bits = 8 * advance_by_bytes as u32;
                        self.vis_clusters.start = self
                            .vis_clusters
                            .end
                            .min(self.vis_clusters.start + advance_by_bits);
                    }
                    bits => {
                        self.cur_byte = Some(BitsIter::new(bits));
                    }
                }
            }

            // Eventually we'll either run out of visleaves
            // or data bytes so this cannot loop infinitely.
        }
    }
}

/// Get an iterator of potentially-visible leaf indices (starting at 1), given a byte array of visdata.
/// The slice should be calculated using the `vis_list` field of `BspLeaf` - if this field is positive,
/// then it is the index to slice the `BspData`'s visdata from.
pub(crate) fn calculate_visdata_indices(vis_data: &[u8], num_clusters: u32) -> VisdataIter<'_> {
    VisdataIter {
        vis_clusters: 0..num_clusters,
        data_bytes: vis_data.iter(),
        cur_byte: None,
    }
}
