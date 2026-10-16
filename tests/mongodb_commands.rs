use lazydb::mongodb_command::{
    MongoCommandError, MongoCommandKind, MongoCommandRisk, parse_command,
};

#[test]
fn preserves_command_and_compound_sort_key_order() {
    let parsed = parse_command(
        r#"{"find":"items","filter":{},"sort":{"z":1,"a":-1}}"#,
        "app",
    )
    .unwrap();
    assert_eq!(parsed.kind, MongoCommandKind::Find);
    assert_eq!(parsed.risk, MongoCommandRisk::ReadOnly);
    let sort = parsed.command.get_document("sort").unwrap();
    assert_eq!(
        sort.keys().map(String::as_str).collect::<Vec<_>>(),
        ["z", "a"]
    );
    assert_eq!(parsed.command_name, "find");
}

#[test]
fn rejects_duplicate_keys_second_command_and_unknown_commands() {
    assert!(
        matches!(parse_command(r#"{"find":"items","filter":{"x":1,"x":2}}"#, "app"), Err(MongoCommandError::DuplicateKey(key)) if key == "x")
    );
    assert_eq!(
        parse_command(r#"{"find":"items","count":"items"}"#, "app").unwrap_err(),
        MongoCommandError::CommandKey
    );
    assert_eq!(
        parse_command(r#"{"eval":"db.items.drop()"}"#, "app").unwrap_err(),
        MongoCommandError::Unsupported("eval".into())
    );
}

#[test]
fn rejects_worker_managed_fields_at_any_depth_and_multiple_documents() {
    for field in [
        "$db",
        "lsid",
        "txnNumber",
        "autocommit",
        "startTransaction",
        "getMore",
        "killCursors",
    ] {
        let input = format!(r#"{{"find":"items","{field}":1}}"#);
        assert!(
            matches!(parse_command(&input, "app"), Err(MongoCommandError::ManagedField(found)) if found == field)
        );
    }
    assert!(matches!(
        parse_command(r#"{"find":"items"} {"ping":1}"#, "app"),
        Err(MongoCommandError::Json(_))
    ));
}

#[test]
fn rejects_excessive_nesting_and_value_count_before_bson_conversion() {
    let mut deeply_nested = r#"{"find":"items","filter":{"x":"#.to_owned();
    deeply_nested.push_str(&"[".repeat(101));
    deeply_nested.push('0');
    deeply_nested.push_str(&"]".repeat(101));
    deeply_nested.push_str("}}}");
    assert!(matches!(
        parse_command(&deeply_nested, "app"),
        Err(MongoCommandError::Json(message)) if message.contains("nesting limit")
    ));

    let many_values = format!(
        r#"{{"find":"items","filter":{{"values":[{}]}}}}"#,
        std::iter::repeat_n("0", 100_001)
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(matches!(
        parse_command(&many_values, "app"),
        Err(MongoCommandError::Json(message)) if message.contains("complexity limit")
    ));
}

#[test]
fn explain_accepts_only_a_supported_read_command_and_inherits_its_namespace() {
    let parsed = parse_command(
        r#"{"explain":{"find":"items","filter":{},"sort":{"z":1,"a":-1}},"verbosity":"executionStats"}"#,
        "app",
    )
    .unwrap();
    assert_eq!(parsed.kind, MongoCommandKind::Explain);
    assert_eq!(parsed.risk, MongoCommandRisk::ReadOnly);
    assert_eq!(parsed.namespaces, ["items"]);

    for command in [
        r#"{"explain":{"insert":"items","documents":[{"x":1}]}}"#,
        r#"{"explain":{"aggregate":"items","pipeline":[{"$out":"archive"}]}}"#,
        r#"{"explain":{"find":"items","exhaust":true}}"#,
        r#"{"explain":{"eval":"db.items.drop()"}}"#,
    ] {
        assert!(parse_command(command, "app").is_err(), "accepted {command}");
    }
}

#[test]
fn write_command_matrix_rejects_unknown_fields_and_malformed_specs() {
    let accepted_insert = parse_command(
        r#"{"insert":"items","documents":[{"x":1}],"ordered":true}"#,
        "app",
    )
    .unwrap();
    assert_eq!(accepted_insert.risk, MongoCommandRisk::Write);
    assert!(accepted_insert.validate_supported_fields().is_ok());

    for command in [
        r#"{"insert":"items","documents":[{"x":1}],"bypassAuthorization":true}"#,
        r#"{"insert":"items","documents":{}}"#,
        r#"{"update":"items","updates":[{"q":{},"u":{"$set":{"x":1}},"unexpected":true}]}"#,
        r#"{"delete":"items","deletes":[{"q":{},"limit":3}]}"#,
        r#"{"createIndexes":"items","indexes":[{"key":{"x":1},"name":"x"}],"unknown":1}"#,
    ] {
        assert!(parse_command(command, "app").is_err(), "accepted {command}");
    }
}

#[test]
fn classifies_destructive_and_aggregate_output_commands() {
    assert_eq!(
        parse_command(r#"{"drop":"items"}"#, "app").unwrap().risk,
        MongoCommandRisk::Destructive
    );
    assert_eq!(
        parse_command(
            r#"{"aggregate":"items","pipeline":[{"$out":"archive"}]}"#,
            "app"
        )
        .unwrap()
        .risk,
        MongoCommandRisk::Write
    );
}

#[test]
fn nested_lookup_and_union_with_namespaces_are_collected() {
    let parsed = parse_command(r#"{"aggregate":"items","pipeline":[{"$lookup":{"from":{"db":"other","coll":"customers"},"as":"c","pipeline":[]}},{"$unionWith":{"db":"archive","coll":"events"}}]}"#, "app").unwrap();
    assert!(parsed.namespaces.contains(&"other.customers".to_owned()));
    assert!(parsed.namespaces.contains(&"archive.events".to_owned()));
}

#[test]
fn literal_data_is_not_classified_as_pipeline_or_session_control() {
    let parsed = parse_command(
        r#"{"aggregate":"items","pipeline":[{"$project":{"data":{"$literal":{"$out":"not-a-namespace","lsid":"ordinary-data"}}}}]}"#,
        "app",
    ).unwrap();
    assert_eq!(parsed.risk, MongoCommandRisk::ReadOnly);
    assert_eq!(parsed.namespaces, ["items"]);
}

#[test]
fn javascript_expressions_are_rejected() {
    for operator in ["$where", "$function", "$accumulator"] {
        let input =
            format!(r#"{{"find":"items","filter":{{"x":{{"{operator}":"return true"}}}}}}"#);
        assert!(
            matches!(parse_command(&input, "app"), Err(MongoCommandError::UnsupportedExpression(found)) if found == operator)
        );
    }
}

#[test]
fn read_preference_and_comment_data_do_not_create_false_write_risk() {
    let parsed = parse_command(
        r#"{"find":"items","comment":"$out should be plain text","filter":{"name":"merge"}}"#,
        "app",
    )
    .unwrap();
    assert_eq!(parsed.risk, MongoCommandRisk::ReadOnly);
}

#[test]
fn capability_field_matrix_allows_reads_and_rejects_unhandled_fields_and_writes() {
    let find = parse_command(
        r#"{"find":"items","filter":{},"sort":{"b":1,"a":1},"limit":20}"#,
        "app",
    )
    .unwrap();
    assert_eq!(
        find.capability().unwrap(),
        lazydb::mongodb_command::MongoCommandCapability::ReadDocuments
    );
    assert!(find.validate_supported_fields().is_ok());

    let unsupported = parse_command(r#"{"find":"items","filter":{},"exhaust":true}"#, "app");
    assert!(
        matches!(unsupported, Err(MongoCommandError::Unsupported(message)) if message.contains("exhaust"))
    );

    let write = parse_command(r#"{"insert":"items","documents":[{"x":1}]}"#, "app").unwrap();
    assert!(write.capability().is_err());
    assert!(write.validate_supported_fields().is_ok());
}

#[test]
fn read_capability_rejects_wrong_field_types_and_negative_limits() {
    assert!(parse_command(r#"{"find":"items","filter":"all"}"#, "app").is_err());
    assert!(parse_command(r#"{"find":"items","limit":-1}"#, "app").is_err());
    assert!(parse_command(r#"{"distinct":"items","key":3}"#, "app").is_err());
}

#[test]
fn read_capabilities_cover_requested_command_families() {
    for (json, capability) in [
        (
            r#"{"count":"items","query":{}}"#,
            lazydb::mongodb_command::MongoCommandCapability::Count,
        ),
        (
            r#"{"distinct":"items","key":"name"}"#,
            lazydb::mongodb_command::MongoCommandCapability::Distinct,
        ),
        (
            r#"{"aggregate":"items","pipeline":[{"$match":{}}]}"#,
            lazydb::mongodb_command::MongoCommandCapability::Aggregate,
        ),
        (
            r#"{"listCollections":1,"nameOnly":true}"#,
            lazydb::mongodb_command::MongoCommandCapability::ListCollections,
        ),
        (
            r#"{"listIndexes":"items"}"#,
            lazydb::mongodb_command::MongoCommandCapability::ListIndexes,
        ),
    ] {
        assert_eq!(
            parse_command(json, "app").unwrap().capability().unwrap(),
            capability
        );
    }
}
