use lazydb::model::redis_preview::RedisPreviewFormatState;
use lazydb::value_preview::{DecodeStatus, PreviewFormat, ValueEncoding, ValueView};

#[test]
fn manual_selection_is_not_automatic() {
    let mut state = RedisPreviewFormatState::default();
    assert!(state.automatic);
    state.select(PreviewFormat {
        encoding: ValueEncoding::Java,
        view: ValueView::Yaml,
    });
    assert!(!state.automatic);
    assert_eq!(state.encoding(), ValueEncoding::Java);
    assert_eq!(state.view(), ValueView::Yaml);
}

#[test]
fn preview_menu_includes_auto_and_serialization_formats() {
    assert_eq!(lazydb::model::redis_preview::format_choices(), 9);
    let mut state = RedisPreviewFormatState::default();
    assert_eq!(state.menu_index(), 0);
    state.select(lazydb::value_preview::PreviewFormat::preset(
        ValueEncoding::Php,
    ));
    assert_eq!(state.menu_index(), 7);
    state.reset_auto();
    assert_eq!(state.menu_index(), 0);
}

#[test]
fn reset_returns_to_raw_auto_mode() {
    let mut state = RedisPreviewFormatState::default();
    state.select(PreviewFormat::HEX);
    state.reset_auto();
    assert_eq!(state.selected, PreviewFormat::RAW);
    assert!(state.automatic);
}

#[test]
fn raw_and_hex_are_byte_views() {
    assert_eq!(
        lazydb::value_preview::decode::decode(b"\xff", PreviewFormat::HEX).unwrap(),
        lazydb::value_preview::decode::DecodedValue::Bytes(vec![0xff])
    );
    assert_eq!(DecodeStatus::NeedsMoreData, DecodeStatus::NeedsMoreData);
}

#[test]
fn json_and_yaml_views_format_valid_structured_text() {
    use lazydb::db::redis::read::{RedisKeyMetadata, RedisPagePosition, RedisType, TtlState};
    use lazydb::db::redis::types::{RedisKeyId, RedisTarget};
    let json = br#"{"name":"\u5f20\u4e09","enabled":true}"#.to_vec();
    let page = lazydb::db::redis::read::RedisValuePage {
        metadata: RedisKeyMetadata {
            key: RedisKeyId {
                target: RedisTarget {
                    profile_id: uuid::Uuid::nil(),
                    database: 0,
                },
                key: b"key".to_vec(),
            },
            value_type: RedisType::String,
            ttl: TtlState::Persistent,
            memory_usage_bytes: None,
            value_size: None,
        },
        position: RedisPagePosition::Complete,
        value: lazydb::db::redis::read::RedisPageValue::String(json),
        truncated: false,
        complete: true,
        raw_bytes: 0,
        formatted_bytes: 0,
    };
    let formatted = lazydb::ui::redis_value::format_page(&page, PreviewFormat::JSON).unwrap();
    assert!(formatted.contains("\n") && formatted.contains("name"));
    let yaml = lazydb::ui::redis_value::format_page(
        &page,
        PreviewFormat {
            encoding: ValueEncoding::Text,
            view: ValueView::Yaml,
        },
    )
    .unwrap();
    assert!(yaml.contains("name:"));
}

#[test]
fn redis_table_keeps_collection_columns_and_raw_identity() {
    let value =
        lazydb::db::redis::read::RedisPageValue::Hash(vec![(b"field".to_vec(), b"value".to_vec())]);
    let table = lazydb::value_preview::table::from_page(&value);
    assert_eq!(table.columns, vec!["Field", "Value"]);
    assert_eq!(table.rows[0].cells, vec!["field", "value"]);
    assert_eq!(table.rows[0].identity[0], b"field");
}

#[test]
fn redis_table_projection_preserves_all_display_rows_and_source_bytes() {
    let value = lazydb::db::redis::read::RedisPageValue::Hash(vec![
        (b"field-1".to_vec(), b"short".to_vec()),
        ("中文".as_bytes().to_vec(), vec![0xff, 0x00]),
    ]);
    let table = lazydb::value_preview::table::from_page(&value);

    assert_eq!(table.columns, vec!["Field", "Value"]);
    assert_eq!(table.rows.len(), 2);
    assert_eq!(table.rows[0].cells, vec!["field-1", "short"]);
    assert_eq!(table.rows[1].cells[0], "中文");
    assert_eq!(table.rows[1].cells[1], r#"\xff\x00"#);
    assert_eq!(table.rows[1].identity[0], "中文".as_bytes());
    assert_eq!(table.rows[1].identity[1], &[0xff, 0x00]);
    assert_eq!(table.rows[1].row_key, "中文".as_bytes());
    assert_eq!(table.rows[1].source_cell(1), Some(&[0xff, 0x00][..]));
}

#[test]
fn unsupported_format_falls_back_without_losing_raw_value() {
    use lazydb::db::redis::read::{RedisKeyMetadata, RedisPagePosition, RedisType, TtlState};
    use lazydb::db::redis::types::{RedisKeyId, RedisTarget};
    let page = lazydb::db::redis::read::RedisValuePage {
        metadata: RedisKeyMetadata {
            key: RedisKeyId {
                target: RedisTarget {
                    profile_id: uuid::Uuid::nil(),
                    database: 0,
                },
                key: b"key".to_vec(),
            },
            value_type: RedisType::String,
            ttl: TtlState::Persistent,
            memory_usage_bytes: None,
            value_size: None,
        },
        position: RedisPagePosition::Complete,
        value: lazydb::db::redis::read::RedisPageValue::String(b"not json".to_vec()),
        truncated: false,
        complete: true,
        raw_bytes: 8,
        formatted_bytes: 8,
    };
    assert!(lazydb::ui::redis_value::format_page(&page, PreviewFormat::JSON).is_err());
    assert_eq!(lazydb::ui::redis_value::page_text(&page), "not json");
}

#[test]
fn java_detection_requires_the_complete_stream_header() {
    assert!(!lazydb::value_preview::java::is_java_serialization(
        b"\xac\xed"
    ));
    assert!(lazydb::value_preview::java::is_java_serialization(
        b"\xac\xed\x00\x05"
    ));
}

#[test]
fn java_parser_decodes_serialized_string() {
    let value = b"\xac\xed\x00\x05t\x00\x05hello";
    let json = lazydb::value_preview::java::parse_java_to_json(value).unwrap();
    assert_eq!(json, "\"hello\"");
}

#[test]
fn php_parser_uses_byte_lengths_and_distinguishes_truncation() {
    let value = b"s:6:\"\xe5\xbc\xa0\xe4\xb8\x89\";";
    let json = lazydb::value_preview::php::parse_php_to_json(value).unwrap();
    assert!(json.contains("张三"));
    let error = lazydb::value_preview::php::parse_php_to_json(b"s:6:\"x").unwrap_err();
    assert_eq!(error.status, DecodeStatus::NeedsMoreData);
}

#[test]
fn php_parser_accepts_empty_arrays() {
    assert_eq!(
        lazydb::value_preview::php::parse_php_to_json(b"a:0:{}").unwrap(),
        "[]"
    );
}

#[test]
fn pickle_parser_handles_protocol_four_scalars() {
    assert!(lazydb::value_preview::pickle::is_pickle_serialization(
        b"\x80\x04N."
    ));
    assert_eq!(
        lazydb::value_preview::pickle::parse_pickle_to_json(b"\x80\x04K\x7f.").unwrap(),
        "127"
    );
}

#[test]
fn pickle_parser_preserves_binary_and_tuple_types() {
    assert_eq!(
        lazydb::value_preview::pickle::parse_pickle_to_json(b"\x80\x04C\x02\x00\xff.").unwrap(),
        "{\n  \"__pickle_type\": \"bytes\",\n  \"length\": 2,\n  \"hex\": \"00ff\"\n}"
    );
    assert!(lazydb::value_preview::pickle::is_pickle_serialization(
        b"N."
    ));
}

#[test]
fn protobuf_parser_preserves_repeated_fields_and_validates_lengths() {
    let data = [0x08, 0x01, 0x08, 0x02, 0x12, 0x03, b'a', b'b', b'c'];
    let json = lazydb::value_preview::protobuf::parse_protobuf_to_json(&data).unwrap();
    assert!(json.contains("field_1"));
    assert!(json.contains("field_2"));
    assert_eq!(
        lazydb::value_preview::protobuf::parse_protobuf_to_json(&[0x12, 0x05])
            .unwrap_err()
            .status,
        DecodeStatus::NeedsMoreData
    );
}

#[test]
fn detection_prefers_validated_json_and_uses_hex_for_unknown_binary() {
    assert_eq!(
        lazydb::value_preview::detect::default_format(br#"{"ok":true}"#, false),
        PreviewFormat::JSON
    );
    assert_eq!(
        lazydb::value_preview::detect::default_format(&[0, 159, 255], false),
        PreviewFormat::HEX
    );
    assert_eq!(
        lazydb::value_preview::detect::default_format(b"hello", false),
        PreviewFormat::RAW
    );
}

#[test]
fn collections_default_to_table_before_binary_detection() {
    assert_eq!(
        lazydb::value_preview::detect::default_format(b"not-json", true),
        PreviewFormat::TABLE
    );
}

#[test]
fn serialization_candidates_are_validated_before_selection() {
    let php = lazydb::value_preview::detect::detect(b"s:5:\"hello\";");
    assert_eq!(
        php.first().map(|candidate| candidate.format.encoding),
        Some(ValueEncoding::Php)
    );
    let invalid_php = lazydb::value_preview::detect::detect(b"s:99:\"x\";");
    assert!(
        invalid_php
            .iter()
            .all(|candidate| candidate.status != DecodeStatus::Complete)
    );
    assert_eq!(
        lazydb::value_preview::detect::default_format(b"s:99:\"x\";", false),
        PreviewFormat::RAW
    );
}

#[test]
fn cell_values_use_the_same_auto_json_pipeline_and_keep_binary_copy() {
    let bytes = br#"{"nested":true}"#;
    let formatted =
        lazydb::ui::redis_value::format_bytes_value(bytes, PreviewFormat::JSON).unwrap();
    assert!(formatted.contains("nested") && formatted.contains('\n'));
    let binary = [0xff, 0x00];
    let hex = lazydb::ui::redis_value::format_bytes_value(&binary, PreviewFormat::HEX).unwrap();
    assert_eq!(hex, "ff00");
}

#[test]
fn unicode_json_preview_keeps_original_text_for_detection_and_formatting() {
    let json = r#"{"name":"界面配置","enabled":true}"#.as_bytes();

    assert_eq!(
        lazydb::value_preview::detect::default_format(json, false),
        PreviewFormat::JSON
    );
    let page = lazydb::db::redis::read::RedisValuePage {
        metadata: lazydb::db::redis::read::RedisKeyMetadata {
            key: lazydb::db::redis::types::RedisKeyId {
                target: lazydb::db::redis::types::RedisTarget {
                    profile_id: uuid::Uuid::nil(),
                    database: 0,
                },
                key: b"unicode-json".to_vec(),
            },
            value_type: lazydb::db::redis::read::RedisType::String,
            ttl: lazydb::db::redis::read::TtlState::Persistent,
            memory_usage_bytes: None,
            value_size: Some(json.len() as u64),
        },
        position: lazydb::db::redis::read::RedisPagePosition::Complete,
        value: lazydb::db::redis::read::RedisPageValue::String(json.to_vec()),
        truncated: false,
        complete: true,
        raw_bytes: json.len(),
        formatted_bytes: json.len(),
    };

    let formatted = lazydb::ui::redis_value::format_page(&page, PreviewFormat::JSON)
        .expect("Unicode JSON must be formatted from original bytes");
    assert!(formatted.contains("界面配置"));
    assert!(!formatted.contains("\\xE7"));
}
