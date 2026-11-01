# MongoDB relation document delete

Feature: the delete key on a row of a MongoDB collection's relation tab removes that document, bounded to its `_id`. Issue: gonzalobuassodev/lazydb#15.

## Specs

S1. The user reported, verbatim: "cada vez que quiero borrar un registro en mongodb", with the toast "Cannot update or delete existing rows without a primary key; no changes were saved". That toast must never be raised from a MongoDB relation row again.

S2. The delete key on the current row of a MongoDB collection's relation tab emits exactly one `Command::DeleteMongoRelationDocument` carrying the tab id, tab generation, connection, namespace and the row's `_id`. It emits no `Command::RelationMutation`.

S3. With no document at the cursor, nothing is emitted.

S4. A read-only MongoDB profile refuses with the existing MongoDB read-only message and emits nothing.

S5. A successful delete notifies the user and reloads the active relation page at the current page number; a failed delete surfaces the error and keeps the page visible.

S6. A result whose tab id or generation is stale is ignored.

S7. The browser tab's `d` delete (`Action::MongoDeleteSelected` → `Runtime::mutate_mongo_delete`) keeps working unchanged.

S8. A visual-mode range delete on a MongoDB relation (`Action::RelationDeleteSelected`) emits nothing and warns that one document is deleted at a time. It must not mark rows for a grid save, because a MongoDB relation never saves through the SQL grid path.

## Tasks

T1 | S1-S4 | `src/action.rs`, `src/app.rs` — command, plan, delete-key branch and the MongoDB early return in `relation_save` | one commit
T2 | S5-S6 | `src/runtime.rs`, `src/db/mongodb/` — dispatch, delete call and result handler | one commit
T3 | S1-S8 | `tests/mongodb_relation.rs` — regression tests, including a test that pins S1 | one commit

## Log

L1. User report, verbatim: "cada vez que quiero borrar un registro en mongodb", with a screenshot of the toast "Cannot update or delete existing rows without a primary key; no changes were saved".

L2. Diagnosis: the relation tab reuses the SQL mutation path. A MongoDB collection reports no SQL columns (`src/model/mongodb_structure.rs:224-273`), so `metadata_fingerprint.primary_key` is empty (`src/db/mutation.rs:150-160`) and the keyless guard at `src/app.rs:29087-29091` refuses the save. The guard is pre-existing and SQL-scoped; the replace path already has a MongoDB-specific analogue (`Command::ReplaceMongoRelationDocument`) and delete has none.

L3. Decision: implement the full MongoDB delete path in the relation tab, mirroring replace. The stopgap (a Mongo-specific message pointing at the browser tab's `d`) and the no-code option were considered and rejected.

L4. Verified while reviewing T1-T3: visual mode is reachable in a MongoDB relation tab (`relation_visual_line` has no driver guard), and with S1's early return in `relation_save` a visual range delete would have marked rows for a save that can never happen, silently. S8 closes that hole and its test pins both the refusal and the no-op save.
