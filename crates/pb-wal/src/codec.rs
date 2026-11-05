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
///
/// v2 added `EventProvenance.ingest_ordinal`. v3 stores `FixedPrice` and
/// `FixedSize` as their raw scaled integers (4 and 8 bytes) instead of
/// length-prefixed decimal strings. bincode is not self-describing, so an
/// older payload cannot be safely decoded into the current struct layout; we
/// reject older versions explicitly rather than risk silent corruption, and
/// operators must drain the WAL before upgrading across a bump.
pub const CURRENT_VERSION: u8 = 3;

/// Encode a `PersistedRecord` into a versioned byte buffer.
///
/// Serializes directly into a single pre-allocated buffer (version byte +
/// bincode payload) to avoid a double-allocation.
pub fn encode(record: &PersistedRecord) -> Result<Vec<u8>, WalError> {
    let size_estimate =
        bincode::serialized_size(record).map_err(|e| WalError::Codec(e.to_string()))? as usize;
    let mut buf = Vec::with_capacity(1 + size_estimate);
    buf.push(CURRENT_VERSION);
    bincode::serialize_into(&mut buf, record).map_err(|e| WalError::Codec(e.to_string()))?;
    Ok(buf)
}

/// Decode a versioned byte buffer back into a `PersistedRecord`.
pub fn decode(data: &[u8]) -> Result<PersistedRecord, WalError> {
    if data.is_empty() {
        return Err(WalError::Codec("empty record".to_string()));
    }
    let version = data[0];
    match version {
        CURRENT_VERSION => {
            bincode::deserialize(&data[1..]).map_err(|e| WalError::Codec(e.to_string()))
        }
        1 | 2 => Err(WalError::Codec(format!(
            "record written by an older binary (codec v{version}); drain the WAL before upgrading"
        ))),
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

    /// A fully-fixed record (no f64, no None-vs-Some ambiguity) for the golden
    /// byte fixture. bincode is positional, so any field reorder/insert in the
    /// persisted types silently changes this layout — the golden test catches it
    ///.
    fn golden_book_record() -> PersistedRecord {
        PersistedRecord::Book(BookEvent {
            asset_id: AssetId::new("tok1"),
            kind: BookEventKind::Delta,
            side: Side::Bid,
            price: FixedPrice::new(5000).unwrap(),
            size: FixedSize::new(100_000_000),
            provenance: EventProvenance {
                recv_timestamp_us: 1_700_000_000_000_000,
                exchange_timestamp_us: 1_699_999_999_000_000,
                source: DataSource::WebSocket,
                source_event_id: Some("evt-1".to_string()),
                source_session_id: None,
                sequence: Some(Sequence::new(42)),
                ingest_ordinal: Some(7),
            },
        })
    }

    #[test]
    fn golden_codec_book_v3_bytes_are_stable() {
        let encoded = encode(&golden_book_record()).unwrap();
        let hex: String = encoded.iter().map(|b| format!("{b:02x}")).collect();
        // Frozen v3 on-disk layout. If a persisted-type field is reordered/added,
        // this fails — forcing a deliberate codec version bump + migration rather
        // than a silent format change.
        const GOLDEN_V3_HEX: &str = "03000000000400000000000000746f6b3101000000000000008813000000e1f5050000000000401e18240a0600c0fd0e18240a0600000000000105000000000000006576742d3100012a00000000000000010700000000000000";
        assert_eq!(hex, GOLDEN_V3_HEX, "WAL codec v3 layout changed");
    }

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
                ingest_ordinal: None,
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
                ingest_ordinal: None,
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
        assert_eq!(encoded[0], CURRENT_VERSION);
    }

    #[test]
    fn older_codec_versions_are_rejected_with_drain_hint() {
        // A v1/v2-tagged payload must be rejected (not silently misparsed into
        // the current layout) so operators drain the WAL before upgrading.
        for version in [1u8, 2] {
            let mut encoded = encode(&test_book_record()).unwrap();
            encoded[0] = version;
            match decode(&encoded).unwrap_err() {
                WalError::Codec(msg) => {
                    assert!(msg.contains("drain"), "unexpected msg: {msg}");
                    assert!(
                        msg.contains(&format!("v{version}")),
                        "unexpected msg: {msg}"
                    );
                }
                other => panic!("expected Codec error, got: {other:?}"),
            }
        }
    }

    // ---- Codec round-trip for all PersistedRecord variants ----

    fn test_trade_record() -> PersistedRecord {
        use pb_types::event::TradeFidelity;
        PersistedRecord::Trade(pb_types::event::TradeEvent {
            asset_id: AssetId::new("tok1"),
            price: FixedPrice::new(5500).unwrap(),
            size: Some(FixedSize::from_f64(50.0).unwrap()),
            side: Some(Side::Ask),
            trade_id: Some("tx-abc".to_string()),
            fidelity: TradeFidelity::Full,
            provenance: EventProvenance {
                recv_timestamp_us: 3_000_000,
                exchange_timestamp_us: 2_999_000,
                source: DataSource::WebSocket,
                source_event_id: Some("hash-1".to_string()),
                source_session_id: None,
                sequence: None,
                ingest_ordinal: None,
            },
        })
    }

    fn test_checkpoint_record() -> PersistedRecord {
        use pb_types::event::PriceLevel;
        PersistedRecord::Checkpoint(pb_types::event::BookCheckpoint {
            asset_id: AssetId::new("tok1"),
            checkpoint_timestamp_us: 4_000_000,
            provenance: EventProvenance {
                recv_timestamp_us: 4_000_000,
                exchange_timestamp_us: 3_999_000,
                source: DataSource::RestSnapshot,
                source_event_id: None,
                source_session_id: None,
                sequence: None,
                ingest_ordinal: None,
            },
            bids: vec![PriceLevel {
                price: FixedPrice::new(5000).unwrap(),
                size: FixedSize::from_f64(100.0).unwrap(),
            }],
            asks: vec![PriceLevel {
                price: FixedPrice::new(5100).unwrap(),
                size: FixedSize::from_f64(200.0).unwrap(),
            }],
            wal_offset: Some(12345),
        })
    }

    fn test_validation_record() -> PersistedRecord {
        use pb_types::event::ReplayMode;
        PersistedRecord::Validation(pb_types::event::ReplayValidation {
            asset_id: AssetId::new("tok1"),
            mode: ReplayMode::RecvTime,
            replay_timestamp_us: 5_000_000,
            reference_timestamp_us: 4_999_000,
            matched: true,
            mismatch_summary: None,
            persisted_at_us: 5_001_000,
        })
    }

    #[test]
    fn encode_decode_roundtrip_trade() {
        let record = test_trade_record();
        let encoded = encode(&record).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(format!("{decoded:?}"), format!("{record:?}"));
    }

    #[test]
    fn encode_decode_roundtrip_checkpoint() {
        let record = test_checkpoint_record();
        let encoded = encode(&record).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(format!("{decoded:?}"), format!("{record:?}"));
    }

    #[test]
    fn encode_decode_roundtrip_validation() {
        let record = test_validation_record();
        let encoded = encode(&record).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(format!("{decoded:?}"), format!("{record:?}"));
    }

    #[test]
    fn decode_truncated_payload_returns_error() {
        let encoded = encode(&test_book_record()).unwrap();
        // Truncate to just the version byte + a few payload bytes.
        let truncated = &encoded[..3.min(encoded.len())];
        let err = decode(truncated).unwrap_err();
        assert!(matches!(err, WalError::Codec(_)));
    }

    #[test]
    fn decode_garbage_payload_returns_error() {
        let mut data = vec![CURRENT_VERSION];
        data.extend_from_slice(&[0xFF; 64]);
        let err = decode(&data).unwrap_err();
        assert!(matches!(err, WalError::Codec(_)));
    }

    #[test]
    fn decode_version_zero_returns_error() {
        let mut encoded = encode(&test_book_record()).unwrap();
        encoded[0] = 0;
        let err = decode(&encoded).unwrap_err();
        match err {
            WalError::Codec(msg) => assert!(msg.contains("unsupported")),
            other => panic!("expected Codec error, got: {other:?}"),
        }
    }

    #[test]
    fn decode_single_version_byte_returns_error() {
        // Just the version byte, no payload.
        let err = decode(&[CURRENT_VERSION]).unwrap_err();
        assert!(matches!(err, WalError::Codec(_)));
    }

    #[test]
    fn all_variants_roundtrip() {
        let records = vec![
            test_book_record(),
            test_ingest_record(),
            test_trade_record(),
            test_checkpoint_record(),
            test_validation_record(),
        ];
        for record in records {
            let encoded = encode(&record).unwrap();
            let decoded = decode(&encoded).unwrap();
            assert_eq!(
                format!("{decoded:?}"),
                format!("{record:?}"),
                "round-trip failed for variant"
            );
        }
    }
}
