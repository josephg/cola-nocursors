#[cfg(feature = "encode")]
mod encode {
    use cola::Replica;

    #[test]
    fn encode_empty() {
        let replica = Replica::new(1, 42);
        let encoded = replica.encode();
        let decoded = Replica::decode(2, &encoded).unwrap();
        assert!(replica.eq_decoded(&decoded));
    }

    #[test]
    #[allow(unused_must_use)]
    fn encode_automerge() {
        let automerge = traces::automerge().chars_to_bytes();

        let mut replica = Replica::new(1, automerge.start_content().len());

        for (start, end, text) in automerge.edits() {
            replica.deleted(start..end);
            replica.inserted(start, text.len());
        }

        let encoded = replica.encode();

        let decoded = Replica::decode(2, &encoded).unwrap();

        assert!(replica.eq_decoded(&decoded));
    }
}
