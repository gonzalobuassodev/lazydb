//! MongoDB collections open in the shared relation tab.
//!
//! T1 (S1): activating a collection in the explorer opens a `RelationTab` whose
//! DATA grid lists the collection's documents. BSON maps onto the existing
//! `CellValue` variants (S5) and paging reuses `FindSpec` skip/limit, so the
//! tab behaves like the SQL table tab it replaces.

mod support;

use lazydb::{
    action::{Action, Command},
    app::App,
    db::{
        ServerInfo,
        catalog::{CatalogEntry, CatalogId, CatalogKind, OptionalMetadata, QualifiedName},
        catalog_mutation::CatalogMutationCapabilities,
        mongodb::{
            read::{find_description, page_result_set, relation_find_spec},
            types::{
                FindSpec, MongoCollectionInfo, MongoCollectionType, MongoDatabaseListing,
                MongoDocument, MongoDocumentPage, MongoExplorerNodeId, MongoNamespace,
            },
        },
        value::CellValue,
    },
    identity::ConnectionIdentity,
    model::{
        execution_target::ExecutionTarget,
        explorer::ExplorerNodeId,
        pagination::{PageRequest, PageSize},
        relation::{RelationRequestKind, RelationView},
        tab::WorkspaceTab,
        workspace::Focus,
    },
    profile::{DatabaseKind, import_connection_url},
};
use uuid::Uuid;

const MONGO_DATABASE: &str = "speedbike_db";
const MONGO_COLLECTION: &str = "bikes";
const SQL_DATABASE: &str = "app";
const SQL_SCHEMA: &str = "public";
const SQL_TABLE: &str = "users";

fn document(bson: mongodb::bson::Document) -> MongoDocument {
    MongoDocument {
        raw: mongodb::bson::raw::RawDocumentBuf::from_bytes(
            mongodb::bson::to_vec(&bson).expect("encodes"),
        )
        .expect("raw document"),
        bson,
        editable: true,
    }
}

fn page(documents: Vec<MongoDocument>, next_offset: Option<u64>) -> MongoDocumentPage {
    MongoDocumentPage {
        documents,
        next_offset,
        truncated: false,
        oversized_document: false,
        bytes: 0,
    }
}

fn mongo_namespace() -> MongoNamespace {
    MongoNamespace::collection(MONGO_DATABASE, MONGO_COLLECTION)
}

// ---------------------------------------------------------------------------
// BSON -> CellValue (S5)
// ---------------------------------------------------------------------------

#[test]
fn bson_values_map_onto_the_existing_cell_value_variants() {
    let object_id = mongodb::bson::oid::ObjectId::new();
    let loaded = page(
        vec![document(mongodb::bson::doc! {
            "_id": object_id,
            "name": "Speedbike",
            "geometry": { "wheels": 2, "frame": "carbon" },
            "tags": ["road", "fast", "light"],
            "stock": 4i64,
            "shelf": 7i32,
            "active": true,
            "retired": mongodb::bson::Bson::Null,
        })],
        None,
    );

    let result = page_result_set(&loaded);

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "_id", "name", "geometry", "tags", "stock", "shelf", "active", "retired"
        ],
        "columns are the union of top-level fields with _id pinned left"
    );
    let row = &result.rows[0];
    assert_eq!(
        row[0],
        CellValue::Text(object_id.to_hex()),
        "an ObjectId cell is its hex text"
    );
    assert_eq!(row[1], CellValue::Text("Speedbike".into()));
    assert_eq!(
        row[2],
        CellValue::Text("{2 fields}".into()),
        "a nested document is a shape marker"
    );
    assert_eq!(
        row[3],
        CellValue::Text("[3]".into()),
        "an array is a shape marker"
    );
    assert_eq!(row[4], CellValue::Integer(4), "an Int64 cell is an Integer");
    assert_eq!(row[5], CellValue::Integer(7), "an Int32 cell is an Integer");
    assert_eq!(row[6], CellValue::Boolean(true), "a bool cell is a Boolean");
    assert_eq!(row[7], CellValue::Null);
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn mongo_relation_preview_pages_through_the_find_spec_and_describes_it() {
    let request = PageRequest::at(PageSize::Ten, 20);

    let spec = relation_find_spec(&mongo_namespace(), request);

    assert_eq!(spec.namespace, mongo_namespace());
    assert_eq!(spec.skip, 20);
    assert_eq!(spec.page_size, PageSize::Ten.get() as u32);
    assert_eq!(
        spec.limit,
        Some(20 + PageSize::Ten.as_u64() + 1),
        "the find asks for one lookahead document so the page can advertise a next page"
    );
    let description = find_description(&spec);
    assert!(
        description.starts_with("db.bikes.find("),
        "the description names the collection it reads, got {description}"
    );
    assert!(
        description.contains(".skip(20)"),
        "the description states the offset it reads, got {description}"
    );
    assert!(
        description.contains(".limit(31)"),
        "the description states the bound it applies, got {description}"
    );
    assert!(
        description.contains(".sort("),
        "the description states the stable sort it applies, got {description}"
    );
}

#[test]
fn a_thousand_row_page_request_is_clamped_to_the_find_budget() {
    // The driver refuses pages above FindSpec::MAX_PAGE_SIZE, so the largest
    // grid page size has to be clamped instead of failing the load.
    let spec = relation_find_spec(
        &mongo_namespace(),
        PageRequest::first(PageSize::OneThousand),
    );

    assert_eq!(spec.page_size, FindSpec::MAX_PAGE_SIZE);
    assert!(spec.validate().is_ok());
}

// ---------------------------------------------------------------------------
// Reducer: activating a collection opens the relation tab
// ---------------------------------------------------------------------------

fn mongo_app() -> (App, Uuid, u64) {
    let profile = import_connection_url("mongodb://localhost:27017/speedbike_db", Some("mongo"))
        .expect("mongodb connection url")
        .profile;
    let profile_id = profile.id;
    let mut app = App::new(vec![profile]);
    let generation = match app.update(Action::RequestConnect(profile_id)).as_slice() {
        [Command::Connect { generation, .. }] => *generation,
        commands => panic!("unexpected commands: {commands:?}"),
    };
    let target = ExecutionTarget::from_profile(
        app.profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .expect("profile"),
    );
    let identity = ConnectionIdentity {
        profile_id,
        generation,
    };
    app.sessions.register_attempt(target, identity);
    let server = ServerInfo {
        kind: DatabaseKind::MongoDb,
        version: "8.0.34".into(),
        database: MONGO_DATABASE.into(),
        current_user: None,
    };
    app.update(Action::ConnectionSucceeded {
        profile_id,
        generation,
        server: server.clone(),
        mutation_capabilities: CatalogMutationCapabilities::default(),
    });
    assert!(
        app.sessions
            .accept_success(identity, server, CatalogMutationCapabilities::default())
    );
    app.update(Action::MongoDatabasesLoaded {
        profile_id,
        generation,
        listing: MongoDatabaseListing {
            databases: vec![MONGO_DATABASE.to_owned()],
            authorized_only: true,
            warning: None,
        },
    });
    app.update(Action::MongoCollectionsLoaded {
        profile_id,
        generation,
        database: MONGO_DATABASE.to_owned(),
        collections: vec![MongoCollectionInfo {
            namespace: mongo_namespace(),
            collection_type: MongoCollectionType::Collection,
            options_ejson: None,
        }],
    });
    app.explorer
        .normalized
        .expanded
        .insert(ExplorerNodeId::Profile(profile_id));
    app.explorer
        .normalized
        .expanded
        .insert(ExplorerNodeId::Mongo {
            profile_id,
            node: MongoExplorerNodeId::Database(MONGO_DATABASE.to_owned()),
        });
    app.focus = Focus::Explorer;
    (app, profile_id, generation)
}

fn collection_node(profile_id: Uuid) -> ExplorerNodeId {
    ExplorerNodeId::Mongo {
        profile_id,
        node: MongoExplorerNodeId::Collection(mongo_namespace()),
    }
}

#[test]
fn activating_a_mongodb_collection_opens_the_relation_tab_on_data() {
    let (mut app, profile_id, _generation) = mongo_app();
    assert!(app.explorer.select_id(collection_node(profile_id)));
    // The first activation expands the collection and loads its indexes.
    app.update(Action::ExplorerOpenSelected);

    let commands = app.update(Action::ExplorerOpenSelected);

    let tab = match app.tabs.last() {
        Some(WorkspaceTab::Relation(tab)) => tab,
        Some(_) => panic!("activating a collection must open a relation tab"),
        None => panic!("activating a collection must open a tab"),
    };
    assert_eq!(app.active_tab, app.tabs.len() - 1);
    assert_eq!(app.focus, Focus::Results);
    assert_eq!(tab.view, RelationView::Data);
    assert_eq!(tab.descriptor.kind, CatalogKind::Table);
    assert_eq!(tab.descriptor.title, MONGO_COLLECTION);
    assert_eq!(
        tab.descriptor.qualified_name,
        QualifiedName {
            database: Some(MONGO_DATABASE.into()),
            schema: None,
            object: MONGO_COLLECTION.into(),
        }
    );
    assert_eq!(
        tab.descriptor.key.object_id,
        CatalogId::new(
            profile_id,
            CatalogKind::Table,
            [MONGO_DATABASE, MONGO_COLLECTION]
        ),
        "the collection is addressed as a table in its native namespace"
    );
    let tab_id = tab.id;
    let key = tab.descriptor.key.clone();
    let page_size = tab.pagination.page_size;
    assert!(
        commands.iter().any(|command| matches!(
            command,
            Command::LoadMongoRelationPreview(request)
                if request.tab_id == tab_id
                    && request.kind == RelationRequestKind::Preview
                    && request.relation == key
                    && request.page == PageRequest::first(page_size)
        )),
        "the first document page must be requested through the find budget, got {commands:?}"
    );
}

// ---------------------------------------------------------------------------
// Regression: SQL tables keep opening exactly as before
// ---------------------------------------------------------------------------

fn sql_app_with_table() -> (App, Uuid, CatalogId) {
    let profile = import_connection_url("postgres://localhost/app", Some("test"))
        .expect("postgres connection url")
        .profile;
    let profile_id = profile.id;
    let mut app = App::new(vec![profile.clone()]);
    app.explorer.normalized.add_profile_with_metadata(
        profile_id,
        "test".into(),
        DatabaseKind::Postgres,
        "localhost".into(),
        lazydb::model::explorer::ProfileProvenance::Saved,
    );
    let database = CatalogEntry::database(
        CatalogId::new(profile_id, CatalogKind::Database, [SQL_DATABASE]),
        QualifiedName {
            database: Some(SQL_DATABASE.into()),
            schema: None,
            object: SQL_DATABASE.into(),
        },
        "database",
        OptionalMetadata::Supported(None),
        true,
    )
    .expect("database entry");
    let schema = CatalogEntry::schema(
        CatalogId::new(profile_id, CatalogKind::Schema, [SQL_DATABASE, SQL_SCHEMA]),
        database.id.clone(),
        QualifiedName {
            database: Some(SQL_DATABASE.into()),
            schema: Some(SQL_SCHEMA.into()),
            object: SQL_SCHEMA.into(),
        },
        "schema",
        OptionalMetadata::Supported(None),
        true,
    )
    .expect("schema entry");
    let table = CatalogEntry::relation(
        CatalogId::new(
            profile_id,
            CatalogKind::Table,
            [SQL_DATABASE, SQL_SCHEMA, SQL_TABLE],
        ),
        schema.id.clone(),
        QualifiedName {
            database: Some(SQL_DATABASE.into()),
            schema: Some(SQL_SCHEMA.into()),
            object: SQL_TABLE.into(),
        },
        "table",
        OptionalMetadata::Supported(None),
        true,
    )
    .expect("table entry");
    let catalog = &mut app
        .explorer
        .normalized
        .profiles
        .get_mut(&profile_id)
        .expect("profile tree")
        .catalog;
    catalog.insert(database).expect("insert database");
    catalog.insert(schema).expect("insert schema");
    catalog.insert(table.clone()).expect("insert table");

    let target = ExecutionTarget {
        profile_id,
        database: SQL_DATABASE.to_owned(),
        schema: Some(SQL_SCHEMA.to_owned()),
    };
    let identity = app
        .sessions
        .start_attempt(target.clone())
        .expect("session attempt");
    let generation = identity.generation;
    let server = ServerInfo {
        kind: DatabaseKind::Postgres,
        version: "17".into(),
        database: SQL_DATABASE.into(),
        current_user: None,
    };
    assert!(
        app.sessions
            .accept_success(identity, server, CatalogMutationCapabilities::default())
    );
    app.connection.profile_id = Some(profile_id);
    app.connection.generation = generation;
    app.connection.status = lazydb::model::workspace::ConnectionStatus::Connected;
    app.connection.target = Some(target);
    app.explorer.normalized.selected = Some(ExplorerNodeId::Catalog(table.id.clone()));
    (app, profile_id, table.id)
}

#[test]
fn activating_a_sql_table_still_opens_the_relation_tab_with_a_sql_preview() {
    let (mut app, _profile_id, table_id) = sql_app_with_table();

    let commands = app.update(Action::ExplorerOpenSelected);

    let tab = match app.tabs.last() {
        Some(WorkspaceTab::Relation(tab)) => tab,
        Some(_) => panic!("activating a sql table must open a relation tab"),
        None => panic!("activating a sql table must open a tab"),
    };
    assert_eq!(tab.view, RelationView::Data);
    assert_eq!(tab.descriptor.key.object_id, table_id);
    let tab_id = tab.id;
    assert!(
        commands.iter().any(|command| matches!(
            command,
            Command::LoadRelationPreview(request)
                if request.tab_id == tab_id && request.relation.object_id == table_id
        )),
        "sql tables keep loading through the relation preview request, got {commands:?}"
    );
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::LoadMongoRelationPreview(_))),
        "sql tables never route through the mongodb loader, got {commands:?}"
    );
}

// ---------------------------------------------------------------------------
// T2 (S4): the status line names the field under the cursor
// ---------------------------------------------------------------------------

fn render_app(app: &App, width: u16, height: u16) -> String {
    use ratatui::{Terminal, backend::TestBackend};

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = lazydb::ui::UiState::new();
    terminal
        .draw(|frame| {
            lazydb::ui::render_with_state_using_icons(
                frame,
                app,
                &mut state,
                lazydb::ui::icons::IconSet::default(),
            )
        })
        .expect("render");
    let buffer = terminal.backend().buffer();
    let mut output = String::new();
    for y in 0..height {
        for x in 0..width {
            output.push_str(buffer[(x, y)].symbol());
        }
        output.push('\n');
    }
    output
}

fn loaded_mongo_relation(app: &mut App, profile_id: Uuid) {
    use lazydb::db::{
        RelationPreview,
        query::{QueryOutcome, QueryStats},
    };
    use lazydb::model::{
        pagination::ResultPagination,
        relation::{OwnedSnapshot, RelationLoad},
    };
    use lazydb::profile::CatalogScope;

    assert!(app.explorer.select_id(collection_node(profile_id)));
    // First activation expands the collection, the second opens the tab.
    app.update(Action::ExplorerOpenSelected);
    app.update(Action::ExplorerOpenSelected);
    let Some(WorkspaceTab::Relation(tab)) = app.tabs.last_mut() else {
        panic!("activating a collection must open a relation tab");
    };
    let page_size = tab.pagination.page_size;
    tab.data = RelationLoad::Ready(OwnedSnapshot::new(
        RelationPreview {
            sql: find_description(&relation_find_spec(
                &mongo_namespace(),
                PageRequest::first(page_size),
            )),
            result: QueryOutcome {
                result_sets: vec![page_result_set(&page(
                    vec![document(mongodb::bson::doc! {
                        "_id": 1_i32,
                        "raceId": "race-1",
                        "position": 3_i64,
                    })],
                    None,
                ))],
                stats: QueryStats::new(Default::default(), Default::default(), 1),
            },
            pagination: ResultPagination::from_page(PageRequest::first(page_size), 1),
            row_versions: None,
        },
        ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        CatalogScope::for_profile(DatabaseKind::MongoDb, MONGO_DATABASE, None),
    ));
}

#[test]
fn mongodb_relation_status_line_names_the_active_field() {
    let (mut app, profile_id, _generation) = mongo_app();
    loaded_mongo_relation(&mut app, profile_id);
    if let Some(WorkspaceTab::Relation(tab)) = app.tabs.last_mut() {
        tab.grid.selected_column = 1;
    }

    let output = render_app(&app, 220, 32);

    assert!(
        output.contains("Find:"),
        "a mongodb snapshot must not be labelled as SQL:\n{output}"
    );
    assert!(
        output
            .lines()
            .flat_map(|line| line.split('│'))
            .any(|pane| pane.contains("raceId") && pane.contains("field:")),
        "the status line must name the field under the cursor:\n{output}"
    );
}
