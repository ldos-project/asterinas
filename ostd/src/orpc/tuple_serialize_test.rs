// SPDX-License-Identifier: MPL-2.0

//! Tests for the `TupleSerialize` derive macro.

#[cfg(ktest)]
mod test {
    use ostd_macros::ktest;
    use serde::Serialize;

    use crate::orpc::TupleSerialize;

    #[derive(TupleSerialize)]
    struct SimpleStruct {
        field1: u64,
        field2: u32,
        field3: u32,
    }

    #[derive(TupleSerialize)]
    struct SingleFieldStruct {
        value: u32,
    }

    #[derive(TupleSerialize)]
    struct EmptyStruct {}

    #[derive(TupleSerialize)]
    struct ParametricStruct<T: Serialize>
    where
        // This bound is in a where clause to test the macro
        T: Copy,
    {
        value: T,
        count: u32,
    }

    #[derive(TupleSerialize)]
    struct BioCompletionStatsMessage {
        latency_us: u64,
        outstanding_pages: u32,
        queue_len: u32,
        request_size_pages: u32,
        device_id: u32,
    }

    fn serialize_to_cbor<T: Serialize>(value: &T) -> alloc::vec::Vec<u8> {
        let mut buffer = alloc::vec::Vec::new();
        value
            .serialize(&mut minicbor_serde::Serializer::new(&mut buffer))
            .unwrap();
        buffer
    }

    /// Helper to generate expected CBOR bytes using minicbor Encode trait to avoid any affects of
    /// serde.
    fn encode_expected<T: minicbor::encode::Encode<()>>(value: T) -> alloc::vec::Vec<u8> {
        minicbor::to_vec(&value).unwrap()
    }

    #[ktest]
    fn simple_struct() {
        let s = SimpleStruct {
            field1: 1,
            field2: 2,
            field3: 3,
        };

        let bytes = serialize_to_cbor(&s);
        let expected = encode_expected((1u64, 2u32, 3u32));
        assert_eq!(bytes, expected);
    }

    #[ktest]
    fn single_field_struct() {
        let s = SingleFieldStruct { value: 42 };

        let bytes = serialize_to_cbor(&s);
        let expected = encode_expected((42u32,));
        assert_eq!(bytes, expected);
    }

    #[ktest]
    fn empty_struct() {
        let s = EmptyStruct {};

        let bytes = serialize_to_cbor(&s);
        let expected = encode_expected(());
        assert_eq!(bytes, expected);
    }

    #[ktest]
    fn parametric_struct() {
        let s = ParametricStruct {
            value: u64::MAX,
            count: 5,
        };

        let bytes = serialize_to_cbor(&s);
        let expected = encode_expected((u64::MAX, 5u32));
        assert_eq!(bytes, expected);
    }

    #[ktest]
    fn bio_completion_stats_message() {
        let msg = BioCompletionStatsMessage {
            latency_us: 1000,
            outstanding_pages: 4,
            queue_len: 8,
            request_size_pages: 2,
            device_id: 1,
        };

        let bytes = serialize_to_cbor(&msg);
        let expected = encode_expected((1000u64, 4u32, 8u32, 2u32, 1u32));
        assert_eq!(bytes, expected);
    }
}
