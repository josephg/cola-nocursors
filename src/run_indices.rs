use std::collections::HashMap;

use crate::*;

/// TODO: docs
#[derive(Clone)]
pub struct RunIndices {
    map: HashMap<ReplicaId, ReplicaIndices>,
    this: ReplicaIndices,
}

impl core::fmt::Debug for RunIndices {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        self.map.fmt(f)
    }
}

impl RunIndices {
    /// TODO: docs
    pub fn assert_invariants(&self, run_tree: &RunTree) {
        for (replica_id, indices) in self.iter() {
            for (run_idx, run_offset, run_len) in indices.splits() {
                let run = run_tree.get_run(run_idx);
                assert_eq!(replica_id, run.replica_id());
                assert_eq!(run_offset, run.start());
                assert_eq!(run_len, run.len());
            }
        }
    }

    /// TODO: docs
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (ReplicaId, &ReplicaIndices)> {
        self.map.iter().map(|(id, indices)| (*id, indices))
    }

    /// TODO: docs
    #[inline]
    pub fn get_mut(&mut self, id: ReplicaId) -> &mut ReplicaIndices {
        &mut self.this
        // self.map.get_mut(&id).unwrap()
    }

    /// TODO: docs
    #[inline]
    pub fn new(id: ReplicaId, idx: LeafIdx<EditRun>, len: Length) -> Self {
        let mut map = HashMap::new();
        map.insert(id, ReplicaIndices::new(idx, len));
        Self { map, this: ReplicaIndices::new(idx, len) }
    }
}

/// TODO: docs
#[derive(Debug, Clone)]
pub struct ReplicaIndices {
    /// TODO: docs
    insertion_runs: Gtree<32, InsertionSplits>,

    /// TODO: docs
    run_idxs: Vec<(LeafIdx<InsertionSplits>, Length)>,

    /// TODO: docs
    last_run: InsertionSplits,
}

// impl core::fmt::Debug for ReplicaIndices {
//     fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
//         self.insertion_runs.fmt(f)
//     }
// }

impl ReplicaIndices {
    #[inline]
    pub fn append(&mut self, len: Length, idx: LeafIdx<EditRun>) {
        let last_split = Split::new(len, idx);
        let new_last = InsertionSplits::new(last_split);
        let old_last = core::mem::replace(&mut self.last_run, new_last);
        self.append_split(old_last);
    }

    #[inline]
    fn append_split(&mut self, splits: InsertionSplits) {
        let last_idx = if self.insertion_runs.is_initialized() {
            self.insertion_runs.append(splits)
        } else {
            self.insertion_runs.initialize(splits)
        };

        let (last_offset, last_len) = self
            .run_idxs
            .last()
            .map(|&(idx, offset)| {
                let splits = self.insertion_runs.get_leaf(idx);
                (offset, splits.len())
            })
            .unwrap_or((0, 0));

        self.run_idxs.push((last_idx, last_offset + last_len));
    }

    #[inline]
    pub fn extend_last(&mut self, extend_by: Length) {
        match &mut self.last_run {
            InsertionSplits::Array { splits, len, total_len } => {
                splits[*len - 1].len += extend_by;
                *total_len += extend_by;
            },

            InsertionSplits::Gtree(splits) => {
                splits.get_last_leaf_mut(|last| {
                    last.len += extend_by;
                });
            },
        }
    }

    #[inline]
    pub fn move_len_to_next_split(
        &mut self,
        insertion_ts: InsertionTimestamp,
        split_at_offset: Length,
        len_moved: Length,
    ) {
        let idx = insertion_ts as usize;

        let (splits, at_offset) = self.splits_at_offset(idx, split_at_offset);

        splits.move_len_to_next_split(at_offset, len_moved);
    }

    #[inline]
    pub fn move_len_to_prev_split(
        &mut self,
        insertion_ts: InsertionTimestamp,
        split_at_offset: Length,
        len_moved: Length,
    ) {
        let idx = insertion_ts as usize;

        let (splits, at_offset) = self.splits_at_offset(idx, split_at_offset);

        splits.move_len_to_prev_split(at_offset, len_moved);
    }

    #[inline]
    pub fn new(first_idx: LeafIdx<EditRun>, len: Length) -> Self {
        let split = Split::new(len, first_idx);

        Self {
            insertion_runs: Gtree::uninit(),
            run_idxs: Vec::with_capacity(128),
            last_run: InsertionSplits::new(split),
        }
    }

    #[inline]
    pub fn split(
        &mut self,
        insertion_ts: InsertionTimestamp,
        at_offset: Length,
        right_idx: LeafIdx<EditRun>,
    ) {
        let idx = insertion_ts as usize;

        let (splits, at_offset) = self.splits_at_offset(idx, at_offset);

        splits.split(at_offset, right_idx);
    }

    #[inline]
    fn splits_at_offset(
        &mut self,
        idx: usize,
        mut at_offset: Length,
    ) -> (&mut InsertionSplits, Length) {
        let splits = if idx == self.run_idxs.len() {
            let offset = self
                .run_idxs
                .last()
                .map(|&(idx, offset)| {
                    let last_split = self.insertion_runs.get_leaf(idx);
                    offset + last_split.len()
                })
                .unwrap_or(0);

            at_offset -= offset;
            &mut self.last_run
        } else {
            let (leaf_idx, run_offset) = self.run_idxs[idx];
            at_offset -= run_offset;
            self.insertion_runs.leaf_mut(leaf_idx)
        };

        (splits, at_offset)
    }

    #[inline]
    pub fn splits(&self) -> Splits<'_> {
        let mut run_splits = self.insertion_runs.leaves();

        let (visited_last, first_split) =
            if let Some(first) = run_splits.next() {
                (false, first)
            } else {
                (true, &self.last_run)
            };

        Splits {
            visited_last,
            current_split: first_split.leaves(),
            last: &self.last_run,
            run_splits,
            offset: 0,
        }
    }
}

/// TODO: docs
const RUN_SPLITS_INLINE: usize = 4;

type InsertionSplits = run_splits::InsertionSplits<RUN_SPLITS_INLINE>;

type RunSplitLeaves<'a> = run_splits::RunSplitLeaves<'a, RUN_SPLITS_INLINE>;

mod run_splits {
    use super::*;

    /// TODO: docs
    #[derive(Clone)]
    pub(super) enum InsertionSplits<const INLINE: usize> {
        /// TODO: docs
        Array { splits: [Split; INLINE], len: usize, total_len: Length },

        /// TODO: docs
        Gtree(Gtree<INLINE, Split>),
    }

    impl<const N: usize> core::fmt::Debug for InsertionSplits<N> {
        fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
            match self {
                Self::Array { splits, len, .. } => splits[..*len].fmt(f),
                Self::Gtree(splits) => splits.fmt(f),
            }
        }
    }

    impl<const INLINE: usize> InsertionSplits<INLINE> {
        #[inline]
        pub fn len(&self) -> Length {
            match self {
                Self::Array { total_len, .. } => *total_len,
                Self::Gtree(splits) => splits.len(),
            }
        }

        #[inline]
        pub fn move_len_to_next_split(
            &mut self,
            at_offset: Length,
            len_move: Length,
        ) {
            debug_assert!(at_offset < self.len());
            debug_assert!(len_move > 0);

            match self {
                Self::Array { splits, .. } => {
                    let mut leaf_idx = 0;
                    let mut next_idx = 0;
                    let mut offset = 0;
                    for (idx, split) in splits.iter().enumerate() {
                        offset += split.len;
                        if offset > at_offset {
                            leaf_idx = idx;
                            next_idx = idx + 1;
                            break;
                        }
                    }
                    let (this, next) =
                        crate::get_two_mut(splits, leaf_idx, next_idx);
                    this.len -= len_move;
                    next.len += len_move;
                },

                Self::Gtree(splits) => {
                    let (leaf_idx, _) = splits.leaf_at_offset(at_offset);
                    let next_idx = splits.get_next_leaf(leaf_idx);
                    splits.with_two_mut(leaf_idx, next_idx, |this, next| {
                        this.len -= len_move;
                        next.len += len_move;
                    });
                },
            }
        }

        #[inline]
        pub fn move_len_to_prev_split(
            &mut self,
            at_offset: Length,
            len_move: Length,
        ) {
            debug_assert!(at_offset < self.len());
            debug_assert!(len_move > 0);

            match self {
                Self::Array { splits, .. } => {
                    let mut prev_idx = 0;
                    let mut leaf_idx = 0;
                    let mut offset = 0;
                    for (idx, split) in splits.iter().enumerate() {
                        offset += split.len;
                        if offset > at_offset {
                            leaf_idx = idx;
                            prev_idx = idx - 1;
                            break;
                        }
                    }
                    let (prev, this) =
                        crate::get_two_mut(splits, prev_idx, leaf_idx);
                    this.len -= len_move;
                    prev.len += len_move;
                },

                Self::Gtree(splits) => {
                    let (leaf_idx, _) = splits.leaf_at_offset(at_offset);
                    let prev_idx = splits.get_prev_leaf(leaf_idx);
                    splits.with_two_mut(prev_idx, leaf_idx, |prev, this| {
                        this.len -= len_move;
                        prev.len += len_move;
                    });
                },
            }
        }

        #[inline]
        pub fn new(first_split: Split) -> Self {
            let mut array = [Split::null(); INLINE];
            let total_len = first_split.len;
            array[0] = first_split;
            Self::Array { splits: array, len: 1, total_len }
        }

        #[inline]
        pub fn split(
            &mut self,
            at_offset: Length,
            right_idx: LeafIdx<EditRun>,
        ) {
            match self {
                InsertionSplits::Array { splits, len, total_len } => {
                    if *len < INLINE {
                        let mut offset = 0;
                        for (idx, split) in splits.iter_mut().enumerate() {
                            offset += split.len;
                            if offset > at_offset {
                                offset -= split.len;
                                let new_split =
                                    split.split(at_offset - offset, right_idx);
                                crate::insert_in_slice(
                                    splits,
                                    new_split,
                                    idx + 1,
                                );
                                *len += 1;
                                return;
                            }
                        }
                        unreachable!();
                    } else {
                        let gtree = Gtree::from_children(
                            splits.iter().copied(),
                            *total_len,
                        );
                        *self = InsertionSplits::Gtree(gtree);
                        self.split(at_offset, right_idx);
                    }
                },

                InsertionSplits::Gtree(splits) => {
                    let (split_idx, split_offset) =
                        splits.leaf_at_offset(at_offset);

                    splits.split_leaf(split_idx, |split| {
                        split.split(at_offset - split_offset, right_idx)
                    });
                },
            };
        }
    }

    impl<const N: usize> gtree::Join for InsertionSplits<N> {}

    impl<const N: usize> gtree::Leaf for InsertionSplits<N> {
        type Length = Length;
    }

    impl<const N: usize> gtree::Summarize for InsertionSplits<N> {
        type Summary = Length;

        #[inline]
        fn summarize(&self) -> Self::Summary {
            self.len()
        }
    }

    impl<const N: usize> InsertionSplits<N> {
        #[inline]
        pub fn leaves(&self) -> RunSplitLeaves<'_, N> {
            match self {
                Self::Array { splits, len, .. } => {
                    RunSplitLeaves::OverArray(splits[..*len].iter())
                },

                Self::Gtree(splits) => {
                    RunSplitLeaves::OverGtree(splits.leaves())
                },
            }
        }
    }

    pub(super) enum RunSplitLeaves<'a, const N: usize> {
        OverArray(core::slice::Iter<'a, Split>),
        OverGtree(gtree::Leaves<'a, N, Split>),
    }

    impl<'a, const N: usize> Iterator for RunSplitLeaves<'a, N> {
        type Item = &'a Split;

        fn next(&mut self) -> Option<Self::Item> {
            match self {
                Self::OverArray(split) => split.next(),
                Self::OverGtree(splits) => splits.next(),
            }
        }
    }
}

/// TODO: docs
#[derive(Copy, Clone)]
struct Split {
    /// TODO: docs
    len: Length,

    /// TODO: docs
    idx_in_run_tree: LeafIdx<EditRun>,
}

impl core::fmt::Debug for Split {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{} @ {:?}", self.len, self.idx_in_run_tree)
    }
}

impl Split {
    #[inline]
    const fn null() -> Self {
        Self { len: 0, idx_in_run_tree: LeafIdx::dangling() }
    }

    #[inline]
    fn new(len: Length, idx: LeafIdx<EditRun>) -> Self {
        Self { idx_in_run_tree: idx, len }
    }

    #[inline]
    fn split(
        &mut self,
        at_offset: Length,
        right_idx: LeafIdx<EditRun>,
    ) -> Self {
        debug_assert!(at_offset < self.len);
        let right_len = self.len - at_offset;
        self.len = at_offset;
        Self { idx_in_run_tree: right_idx, len: right_len }
    }
}

impl gtree::Summarize for Split {
    type Summary = Length;

    #[inline]
    fn summarize(&self) -> Self::Summary {
        self.len
    }
}

impl gtree::Join for Split {}

impl gtree::Leaf for Split {
    type Length = Length;
}

pub use splits::Splits;

mod splits {
    use super::*;

    pub struct Splits<'a> {
        pub(super) run_splits: gtree::Leaves<'a, 32, InsertionSplits>,
        pub(super) current_split: RunSplitLeaves<'a>,
        pub(super) last: &'a InsertionSplits,
        pub(super) offset: Length,
        pub(super) visited_last: bool,
    }

    impl<'a> Iterator for Splits<'a> {
        type Item = (LeafIdx<EditRun>, Length, Length); // (idx, offset, len)

        fn next(&mut self) -> Option<Self::Item> {
            if let Some(split) = self.current_split.next() {
                let idx = split.idx_in_run_tree;
                let len = split.len;
                let offset = self.offset;
                self.offset += len;
                Some((idx, offset, len))
            } else if let Some(splits) = self.run_splits.next() {
                self.current_split = splits.leaves();
                self.next()
            } else if self.visited_last {
                None
            } else {
                self.visited_last = true;
                self.current_split = self.last.leaves();
                self.next()
            }
        }
    }
}