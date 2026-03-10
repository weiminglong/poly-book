//! Version-prefixed bincode serialization for WAL records.
//!
//! Every WAL payload is prefixed with a single version byte, allowing
//! the on-disk format to evolve without breaking existing segments.
//!
//! ```text
//! ┌──────────┬──────────────────────────┐
//! │ ver: u8  │ bincode payload: [u8]    │
//! └──────────┴──────────────────────────┘
//! ```

use pb_types::event::PersistedRecord;

use crate::WalError;

/// Current serialization version.
const CURRENT_VERSION: u8 = 1;

/// Encode a `PersistedRecord` into a versioned byte buffer.
pub fn encode(record: &PersistedRecord) -> Result<Vec<u8>, WalError> {
    let bincode_bytes = bincode::serialize(record).map_err(|e| WalError::Codec(e.to_string()))?;
    let mut buf = Vec::with_capacity(1 + bincode_bytes.len());
    buf.push(CURRENT_VERSION);
    buf.extend_from_slice(&bincode_bytes);
    Ok(buf)
}

/// Decode a versioned byte buffer back into a `PersistedRecord`.
pub fn decode(data: &[u8]) -> Result<PersistedRecord, WalError> {
    if data.is_empty() {
        return Err(WalError::Codec("empty record".to_string()));
    }
    let version = data[0];
    match version {
        1 => bincode::deserialize(&data[1..]).map_err(|e| WalError::Codec(e.to_string())),
        other => Err(WalError::Codec(format!(
            "unsupported record version: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use pb_types::event::{
        BookEvent, BookEventKind, DataSource, EventProvenance, IngestEvent, IngestEventKind,
        PersistedRecord, Side,
    };
    use pb_types::{AssetId, FixedPrice, FixedSize, Sequence};

    use super::*;

    fn test_book_record() -> PersistedRecord {
        PersistedRecord::Book(BookEvent {
            asset_id: AssetId::new("tok1"),
            kind: BookEventKind::Delta,
            side: Side::Bid,
            price: FixedPrice::new(5000).unwrap(),
            size: FixedSize::from_f64(100.0).unwrap(),
            provenance: EventProvenance {
                recv_timestamp_us: 1_000_000,
                exchange_timestamp_us: 999_000,
                source: DataSource::WebSocket,
                source_event_id: None,
                source_session_id: Some("ws-1".to_string()),
                sequence: Some(Sequence::new(42)),
            },
        })
    }

    fn test_ingest_record() -> PersistedRecord {
        PersistedRecord::Ingest(IngestEvent {
            asset_id: Some(AssetId::new("tok1")),
            kind: IngestEventKind::SequenceGap,
            provenance: EventProvenance {
                recv_timestamp_us: 2_000_000,
                exchange_timestamp_us: 1_999_000,
                source: DataSource::WebSocket,
                source_event_id: None,
                source_session_id: None,
                sequence: None,
            },
            expected_sequence: Some(5),
            observed_sequence: Some(8),
            details: Some("gap detected".to_string()),
        })
    }

    #[test]
    fn encode_decode_roundtrip_book() {
        let record = test_book_record();
        let encoded = encode(&record).unwrap();
        assert_eq!(encoded[0], CURRENT_VERSION);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(format!("{decoded:?}"), format!("{record:?}"));
    }

    #[test]
    fn encode_decode_roundtrip_ingest() {
        let record = test_ingest_record();
        let encoded = encode(&record).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(format!("{decoded:?}"), format!("{record:?}"));
    }

    #[test]
    fn decode_empty_returns_error() {
        let err = decode(&[]).unwrap_err();
        assert!(matches!(err, WalError::Codec(_)));
    }

    #[test]
    fn decode_unknown_version_returns_error() {
        let mut encoded = encode(&test_book_record()).unwrap();
        encoded[0] = 255; // unsupported version
        let err = decode(&encoded).unwrap_err();
        match err {
            WalError::Codec(msg) => assert!(msg.contains("unsupported")),
            other => panic!("expected Codec error, got: {other:?}"),
        }
    }

    #[test]
    fn version_byte_is_first() {
        let encoded = encode(&test_book_record()).unwrap();
        assert!(encoded.len() > 1);
        assert_eq!(encoded[0], 1);
    }
}
