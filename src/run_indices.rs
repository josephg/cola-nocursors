use core::ops::{Index, IndexMut};

use crate::*;

/// A data structure used when merging remote edits to efficiently map
/// an [`Anchor`] to the [`LeafIdx`] of the [`EditRun`] that contains it.
#[derive(Clone, PartialEq)]
#[cfg_attr(feature = "encode", derive(serde::Serialize, serde::Deserialize))]
pub(crate) struct RunIndices {
    map: ReplicaIdMap<ReplicaIndices>,
}

impl core::fmt::Debug for RunIndices {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        self.map.fmt(f)
    }
}

/// TODO: docs
#[derive(PartialEq, Eq)]
pub(crate) enum AnchorBias {
    Left,
    Right,
}

impl RunIndices {
    pub fn assert_invariants(&self, run_tree: &RunTree) {
        for (&replica_id, indices) in self.map.iter() {
            indices.assert_invariants();

            let mut offset = 0;

            for (idx, splits) in indices.splits().enumerate() {
                for split in splits.leaves() {
                    let run = run_tree.run(split.idx);
                    assert_eq!(replica_id, run.replica_id());
                    assert_eq!(split.len, run.len());
                    assert_eq!(offset, run.start());
                    assert_eq!(idx, run.run_ts() as usize);
                    offset += split.len;
                }
            }
        }
    }

    #[inline]
    pub fn get_mut(&mut self, id: ReplicaId) -> &mut ReplicaIndices {
        self.map.entry(id).or_insert_with(ReplicaIndices::new)
    }

    /// Returns the [`LeafIdx`] of the [`EditRun`] that contains the given
    /// [`Anchor`].
    #[inline]
    pub fn idx_at_anchor(
        &self,
        anchor: Anchor,
        anchor_ts: RunTs,
        bias: AnchorBias,
    ) -> LeafIdx<EditRun> {
        self.map.get(&anchor.replica_id()).unwrap().idx_at_offset(
            anchor_ts,
            anchor.offset(),
            bias,
        )
    }

    #[inline]
    pub fn new() -> Self {
        Self { map: ReplicaIdMap::default() }
    }
}

/// Contains the [`LeafIdx`]s of all the [`EditRun`]s that have been inserted
/// by a given `Replica`.
#[derive(Clone, PartialEq)]
#[cfg_attr(feature = "encode", derive(serde::Serialize, serde::Deserialize))]
pub(crate) struct ReplicaIndices {
    /// The [`Fragments`] are stored sequentially and in order of insertion.
    ///
    /// When a new [`EditRun`] is created we append a new [`Fragments`] to the
    /// vector. As long as the following insertions continue that run we simply
    /// increase the length of the last [`Fragments`].
    ///
    /// Once that run ends we append new [`Fragments`], and so on.
    vec: Vec<(Fragments, Length)>,
}

impl core::fmt::Debug for ReplicaIndices {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.debug_list()
            .entries(self.vec.iter().map(|(splits, _)| splits))
            .finish()
    }
}

/// We can use the `RunTs` as an index into the `vec` because it is only
/// incremented when the current run is interrupted and a new one is started.
///
/// Using the `RunTs` as an index allows us to find the `Fragments`
/// corresponding to a given offset in `O(1)` instead of having to do a binary
/// search.
impl Index<RunTs> for ReplicaIndices {
    type Output = (Fragments, Length);

    #[inline]
    fn index(&self, run_ts: RunTs) -> &Self::Output {
        &self.vec[run_ts as usize]
    }
}

impl IndexMut<RunTs> for ReplicaIndices {
    #[inline]
    fn index_mut(&mut self, run_ts: RunTs) -> &mut Self::Output {
        &mut self.vec[run_ts as usize]
    }
}

impl ReplicaIndices {
    #[inline]
    pub fn append(&mut self, len: Length, idx: LeafIdx<EditRun>) {
        let fragment = Fragment::new(len, idx);

        let new_last = Fragments::new(fragment);

        let (last_offset, last_len) = self
            .vec
            .last()
            .map(|(fragments, offset)| (fragments.len(), *offset))
            .unwrap_or((0, 0));

        self.vec.push((new_last, last_offset + last_len));
    }

    #[inline]
    pub fn append_to_last(&mut self, len: Length, idx: LeafIdx<EditRun>) {
        let split = Fragment::new(len, idx);
        self.vec.last_mut().unwrap().0.append(split);
    }

    fn assert_invariants(&self) {
        let mut offset = 0;

        for &(ref splits, splits_offset) in self.vec.iter() {
            assert_eq!(splits_offset, offset);
            splits.assert_invariants();
            offset += splits.len();
        }
    }

    #[inline]
    pub fn extend_last(&mut self, extend_by: Length) {
        self.vec.last_mut().unwrap().0.extend(extend_by);
    }

    #[inline]
    fn idx_at_offset(
        &self,
        run_ts: RunTs,
        at_offset: Length,
        bias: AnchorBias,
    ) -> LeafIdx<EditRun> {
        let (splits, offset) = &self[run_ts];
        splits.fragment_at_offset(at_offset - offset, bias).idx
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.vec.len()
    }

    #[inline]
    pub fn move_len_to_next_split(
        &mut self,
        run_ts: RunTs,
        split_at_offset: Length,
        len_moved: Length,
    ) {
        let (splits, offset) = &mut self[run_ts];
        splits.move_len_to_next_fragment(split_at_offset - *offset, len_moved);
    }

    #[inline]
    pub fn move_len_to_prev_split(
        &mut self,
        run_ts: RunTs,
        split_at_offset: Length,
        len_moved: Length,
    ) {
        let (splits, offset) = &mut self[run_ts];
        splits.move_len_to_prev_split(split_at_offset - *offset, len_moved);
    }

    #[inline]
    fn new() -> Self {
        Self { vec: Vec::new() }
    }

    #[inline]
    pub fn split(
        &mut self,
        run_ts: RunTs,
        at_offset: Length,
        right_idx: LeafIdx<EditRun>,
    ) {
        let (splits, offset) = &mut self[run_ts];
        splits.split(at_offset - *offset, right_idx);
    }

    #[inline]
    fn splits(&self) -> impl Iterator<Item = &Fragments> {
        self.vec.iter().map(|(splits, _)| splits)
    }
}

const FRAGMENTS_INLINE: usize = 8;

type Fragments = fragments::Fragments<FRAGMENTS_INLINE>;

mod fragments {
    use super::*;

    /// The `Fragment`s that an insertion run has been fragmented into.
    #[derive(Clone, PartialEq)]
    #[cfg_attr(
        feature = "encode",
        derive(serde::Serialize, serde::Deserialize)
    )]
    pub(crate) enum Fragments<const INLINE: usize> {
        /// The first `INLINE` fragments are stored inline to avoid
        /// allocating a `Gtree` for runs that are not heavily fragmented.
        Array(Array<INLINE>),

        /// Once the number of fragments exceeds `INLINE` we switch to a
        /// `Gtree`.
        Gtree(Gtree<INLINE, Fragment>),
    }

    impl<const N: usize> core::fmt::Debug for Fragments<N> {
        fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
            match self {
                Self::Array(array) => array.fmt(f),

                Self::Gtree(gtree) => f
                    .debug_list()
                    .entries(gtree.leaves_from_first().map(|(_, split)| split))
                    .finish(),
            }
        }
    }

    impl<const INLINE: usize> Fragments<INLINE> {
        pub fn assert_invariants(&self) {
            match self {
                Self::Array(array) => array.assert_invariants(),
                Self::Gtree(gtree) => gtree.assert_invariants(),
            }
        }

        #[inline]
        pub fn append(&mut self, fragment: Fragment) {
            match self {
                Fragments::Array(array) => {
                    if array.len == INLINE {
                        let mut gtree = Gtree::from_leaves(
                            array.fragments().iter().copied(),
                            array.total_len,
                        );
                        gtree.append(fragment);
                        *self = Self::Gtree(gtree);
                    } else {
                        array.append(fragment);
                    }
                },

                Fragments::Gtree(gtree) => {
                    gtree.append(fragment);
                },
            }
        }

        #[inline]
        pub fn extend(&mut self, extend_by: Length) {
            match self {
                Fragments::Array(array) => {
                    array.extend_last(extend_by);
                },

                Fragments::Gtree(gtree) => {
                    gtree.with_last_leaf_mut(|last| {
                        last.len += extend_by;
                    });
                },
            }
        }

        #[inline]
        pub fn fragment_at_offset(
            &self,
            at_offset: Length,
            bias: AnchorBias,
        ) -> &Fragment {
            debug_assert!(
                at_offset < self.len()
                    || at_offset == self.len() && bias == AnchorBias::Left
            );

            match self {
                Self::Array(array) => {
                    array.fragment_at_offset(at_offset, bias)
                },

                Self::Gtree(gtree) => {
                    let (leaf_idx, fragment_offset) =
                        gtree.leaf_at_offset(at_offset);

                    let fragment = gtree.leaf(leaf_idx);

                    if fragment_offset + fragment.len == at_offset
                        && bias == AnchorBias::Right
                    {
                        gtree.leaf(gtree.next_leaf(leaf_idx).unwrap())
                    } else {
                        fragment
                    }
                },
            }
        }

        #[inline]
        pub fn len(&self) -> Length {
            match self {
                Self::Array(array) => array.total_len,
                Self::Gtree(splits) => splits.len(),
            }
        }

        #[inline]
        pub fn move_len_to_next_fragment(
            &mut self,
            fragment_at_offset: Length,
            len_move: Length,
        ) {
            debug_assert!(fragment_at_offset < self.len());
            debug_assert!(len_move > 0);

            match self {
                Self::Array(array) => {
                    array.move_len_to_next_fragment(
                        fragment_at_offset,
                        len_move,
                    );
                },

                Self::Gtree(gtree) => {
                    let (leaf_idx, _) =
                        gtree.leaf_at_offset(fragment_at_offset);

                    let next_idx = gtree.next_leaf(leaf_idx).unwrap();

                    gtree.with_two_mut(leaf_idx, next_idx, |this, next| {
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
                Self::Array(array) => {
                    array.move_len_to_prev_fragment(at_offset, len_move)
                },

                Self::Gtree(gtree) => {
                    let (leaf_idx, _) = gtree.leaf_at_offset(at_offset);

                    let prev_idx = gtree.prev_leaf(leaf_idx).unwrap();

                    gtree.with_two_mut(prev_idx, leaf_idx, |prev, this| {
                        this.len -= len_move;
                        prev.len += len_move;
                    });
                },
            }
        }

        #[inline]
        pub fn new(first_split: Fragment) -> Self {
            let mut array = [Fragment::null(); INLINE];
            let total_len = first_split.len;
            array[0] = first_split;
            Self::Array(Array { fragments: array, len: 1, total_len })
        }

        #[inline]
        pub fn split(&mut self, at_offset: Length, new_idx: LeafIdx<EditRun>) {
            match self {
                Fragments::Array(array) => {
                    if array.len == INLINE {
                        let gtree = Gtree::from_leaves(
                            array.fragments().iter().copied(),
                            array.total_len,
                        );
                        *self = Fragments::Gtree(gtree);
                        self.split(at_offset, new_idx);
                    } else {
                        array.split(at_offset, new_idx);
                    }
                },

                Fragments::Gtree(gtree) => {
                    let (fragment_idx, fragment_offset) =
                        gtree.leaf_at_offset(at_offset);

                    gtree.split_leaf(fragment_idx, |fragment| {
                        fragment.split(at_offset - fragment_offset, new_idx)
                    });
                },
            };
        }
    }

    #[derive(Clone, PartialEq)]
    pub(crate) struct Array<const N: usize> {
        fragments: [Fragment; N],

        /// The number of non-null `Fragment`s in the array.
        len: usize,

        /// The total length of all `Fragment`s in the array.
        total_len: Length,
    }

    impl<const N: usize> core::fmt::Debug for Array<N> {
        fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
            self.fragments().fmt(f)
        }
    }

    impl<const N: usize> Array<N> {
        #[inline]
        fn assert_invariants(&self) {
            let mut total_len = 0;

            for fragment in &self.fragments[0..self.len] {
                total_len += fragment.len;
                assert!(!fragment.is_null());
            }

            assert_eq!(self.total_len, total_len);

            for fragment in &self.fragments[self.len..] {
                assert!(fragment.is_null());
            }
        }

        #[inline]
        fn append(&mut self, fragment: Fragment) {
            debug_assert!(self.len < N);

            self.total_len += fragment.len;
            self.fragments[self.len] = fragment;
            self.len += 1;
        }

        #[inline]
        fn extend_last(&mut self, extend_by: Length) {
            self.fragments[self.len - 1].len += extend_by;
            self.total_len += extend_by;
        }

        #[inline]
        fn fragment_at_offset(
            &self,
            at_offset: Length,
            bias: AnchorBias,
        ) -> &Fragment {
            let (idx, fragment_offset) = self.idx_at_offset(at_offset);
            let fragment = &self.fragments[idx];
            if fragment_offset + fragment.len == at_offset
                && bias == AnchorBias::Right
            {
                &self.fragments[idx + 1]
            } else {
                fragment
            }
        }

        #[inline]
        fn fragments(&self) -> &[Fragment] {
            &self.fragments[..self.len]
        }

        #[inline]
        fn fragments_mut(&mut self) -> &mut [Fragment] {
            &mut self.fragments[..self.len]
        }

        #[inline]
        fn idx_at_offset(&self, at_offset: Length) -> (usize, Length) {
            let mut offset = 0;
            for (idx, fragment) in self.fragments().iter().enumerate() {
                offset += fragment.len;
                if offset >= at_offset {
                    return (idx, offset - fragment.len);
                }
            }
            unreachable!();
        }

        #[inline]
        fn move_len_to_next_fragment(
            &mut self,
            fragment_at_offset: Length,
            len_move: Length,
        ) {
            let (this, _) = self.idx_at_offset(fragment_at_offset);
            let next = this + 1;
            let (this, next) =
                crate::get_two_mut(self.fragments_mut(), this, next);
            this.len -= len_move;
            next.len += len_move;
        }

        #[inline]
        fn move_len_to_prev_fragment(
            &mut self,
            fragment_at_offset: Length,
            len_move: Length,
        ) {
            let (this, _) = self.idx_at_offset(fragment_at_offset);
            let prev = this - 1;
            let (prev, this) =
                crate::get_two_mut(self.fragments_mut(), prev, this);
            this.len -= len_move;
            prev.len += len_move;
        }

        #[inline]
        fn split(&mut self, at_offset: Length, new_idx: LeafIdx<EditRun>) {
            let (idx, fragment_offset) = self.idx_at_offset(at_offset);

            self.len += 1;

            let fragments = &mut self.fragments[0..self.len];

            let fragment = &mut fragments[idx];

            let new_fragment =
                fragment.split(at_offset - fragment_offset, new_idx);

            crate::insert_in_slice(fragments, new_fragment, idx + 1);
        }
    }

    #[cfg(feature = "encode")]
    mod array_serde {
        use serde::ser::SerializeMap;
        use serde::{de, ser};

        use super::*;

        impl<const N: usize> ser::Serialize for Array<N> {
            fn serialize<S: ser::Serializer>(
                &self,
                serializer: S,
            ) -> Result<S::Ok, S::Error> {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("fragments", self.fragments())?;
                map.serialize_entry("len", &self.len)?;
                map.serialize_entry("total_len", &self.total_len)?;
                map.end()
            }
        }

        impl<'de, const N: usize> de::Deserialize<'de> for Array<N> {
            fn deserialize<D: de::Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Self, D::Error> {
                struct ArrayVisitor<const N: usize>;

                impl<'de, const N: usize> de::Visitor<'de> for ArrayVisitor<N> {
                    type Value = Array<N>;

                    #[inline]
                    fn expecting(
                        &self,
                        formatter: &mut core::fmt::Formatter,
                    ) -> core::fmt::Result {
                        formatter.write_str("a map representing an Array")
                    }

                    #[inline]
                    fn visit_map<V: de::MapAccess<'de>>(
                        self,
                        mut map: V,
                    ) -> Result<Self::Value, V::Error> {
                        let mut len = None;
                        let mut total_len = None;
                        let mut fragments_vec = None;

                        while let Some(key) = map.next_key()? {
                            match key {
                                "len" => {
                                    if len.is_some() {
                                        return Err(
                                            de::Error::duplicate_field("len"),
                                        );
                                    }
                                    len = Some(map.next_value()?);
                                },

                                "total_len" => {
                                    if total_len.is_some() {
                                        return Err(
                                            de::Error::duplicate_field(
                                                "total_len",
                                            ),
                                        );
                                    }
                                    total_len = Some(map.next_value()?);
                                },

                                "fragments" => {
                                    if fragments_vec.is_some() {
                                        return Err(
                                            de::Error::duplicate_field(
                                                "fragments",
                                            ),
                                        );
                                    }
                                    fragments_vec = Some(
                                        map.next_value::<Vec<Fragment>>()?,
                                    );
                                },

                                _ => {
                                    return Err(de::Error::unknown_field(
                                        key,
                                        &["fragments", "len", "total_len"],
                                    ));
                                },
                            }
                        }

                        let len = len
                            .ok_or_else(|| de::Error::missing_field("len"))?;

                        let total_len = total_len.ok_or_else(|| {
                            de::Error::missing_field("total_len")
                        })?;

                        let fragments_vec =
                            fragments_vec.ok_or_else(|| {
                                de::Error::missing_field("fragments")
                            })?;

                        if fragments_vec.len() != len {
                            return Err(de::Error::invalid_length(
                                fragments_vec.len(),
                                &len.to_string().as_str(),
                            ));
                        }

                        if fragments_vec.len() > N {
                            return Err(de::Error::invalid_length(
                                fragments_vec.len(),
                                &format!("no more than {N}").as_str(),
                            ));
                        }

                        let mut fragments = [Fragment::null(); N];

                        fragments[..len]
                            .copy_from_slice(fragments_vec.as_slice());

                        Ok(Array { fragments, len, total_len })
                    }
                }

                deserializer.deserialize_map(ArrayVisitor)
            }
        }
    }

    impl<const N: usize> gtree::Join for Fragments<N> {}

    impl<const N: usize> gtree::Leaf for Fragments<N> {
        type Length = Length;

        #[inline]
        fn len(&self) -> Self::Length {
            self.len()
        }
    }

    impl<const N: usize> Fragments<N> {
        #[inline]
        pub fn leaves(&self) -> RunSplitLeaves<'_> {
            match self {
                Self::Array(array) => {
                    let iter = Box::new(array.fragments().iter()) as _;
                    RunSplitLeaves { iter }
                },

                Self::Gtree(gtree) => {
                    let iter = Box::new(
                        gtree.leaves_from_first().map(|(_idx, leaf)| leaf),
                    ) as _;
                    RunSplitLeaves { iter }
                },
            }
        }
    }

    pub(crate) struct RunSplitLeaves<'a> {
        iter: Box<dyn Iterator<Item = &'a Fragment> + 'a>,
    }

    impl<'a> Iterator for RunSplitLeaves<'a> {
        type Item = &'a Fragment;

        fn next(&mut self) -> Option<Self::Item> {
            self.iter.next()
        }
    }
}

/// The length and [`LeafIdx`] of a fragment of a single insertion run.
#[derive(Copy, Clone, PartialEq)]
#[cfg_attr(feature = "encode", derive(serde::Serialize, serde::Deserialize))]
pub(crate) struct Fragment {
    len: Length,
    idx: LeafIdx<EditRun>,
}

impl core::fmt::Debug for Fragment {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{} @ {:?}", self.len, self.idx)
    }
}

impl Fragment {
    #[inline]
    const fn null() -> Self {
        Self { len: 0, idx: LeafIdx::dangling() }
    }

    #[inline]
    fn is_null(&self) -> bool {
        *self == Self::null()
    }

    #[inline]
    fn new(len: Length, idx: LeafIdx<EditRun>) -> Self {
        Self { idx, len }
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
        Self { idx: right_idx, len: right_len }
    }
}

impl gtree::Join for Fragment {}

impl gtree::Leaf for Fragment {
    type Length = Length;

    #[inline]
    fn len(&self) -> Self::Length {
        self.len
    }
}
