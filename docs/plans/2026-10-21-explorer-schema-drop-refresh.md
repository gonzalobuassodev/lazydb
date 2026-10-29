# Explorer Schema Drop Refresh Implementation Plan

**Goal:** Keep unchanged Explorer catalog subtrees visible while schema deletion and cascade reconciliation refresh cached pages.

**Architecture:** Merge refresh pages by stable catalog identity, retain the previous tree until all pages of a paginated refresh arrive, and remove only identities absent from the completed result. Hide per-owner loading rows when a previous snapshot is available. Distinguish shallow schema-list refreshes from normal cascading refreshes, and limit CASCADE reconciliation to cached groups, relations, and relation-child metadata.

**Tech Stack:** Rust 2024, Tokio command runtime, Explorer catalog reducer.

---

### Task 1: Preserve catalog snapshots during paginated refresh

**Files:**
- Modify: `src/model/explorer.rs`
- Modify: `src/app.rs`

Refresh requests with a loaded or stale snapshot stage returned child IDs across pages. Reconcile only after the final page, retaining stable subtrees and removing children absent from the completed result. On refresh failure, retain cached data and discard the pending reconciliation set.

### Task 2: Keep cached owners visually stable

**Files:**
- Modify: `src/model/explorer.rs`

When an owner is refreshing and its previous state is loaded or stale, suppress its transient `Loading...` row while continuing to display cached children. Owners without a snapshot retain the normal loading row.

### Task 3: Scope cascade refresh and reject stale schema-list results

**Files:**
- Modify: `src/app.rs`
- Modify: `src/model/explorer.rs`

After a successful schema drop, remove the schema locally and shallow-refresh its parent list. Reconcile stable schema identities without reloading their descendants. For PostgreSQL cascade reconciliation, refresh group summaries, loaded relation lists, and loaded relation-child metadata only for owners that already have snapshots; do not start requests for untouched groups or relations. Normal database refreshes continue through already-loaded descendants to discover SQL-created objects.

### Verification

Verification: `cargo fmt --all`, `cargo check --tests`, the focused `catalog_reducer`, `explorer_state`, `ui_render`, `clickhouse_catalog_drop`, and `catalog_contract` test targets, plus the full `cargo test --tests` suite.
