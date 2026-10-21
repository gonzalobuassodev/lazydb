# SQL Server Relation Preview PR Implementation Plan

> **For Claude:** Execute this plan task by task in the dedicated PR worktree.

**Goal:** Complete PR #17 with table and view regression coverage, reliable fixture cleanup, and verified CI before merging it to close issue #16.

**Architecture:** Keep the contributor's adapter fix: previews accept three- and four-component relation identities, while DDL requires a numeric object ID. Exercise both catalog object groups through the existing SQL Server integration test and reuse its existing panic-catching cleanup pattern. Keep release preparation limited to an Unreleased changelog entry.

**Tech Stack:** Rust, Tokio, Tiberius, Cargo, GitHub Actions, GitHub CLI.

---

### Task 1: Complete the regression fixture

**Files:**
- Modify: `tests/sqlserver_adapter.rs`
- Modify: `src/db/mssql.rs` (compatibility comments only)

1. Create a uniquely named schema, table, and view, quoting every identifier.
2. Include setup and assertions inside `AssertUnwindSafe(async { ... }).catch_unwind()` so cleanup also runs after setup or assertion failures.
3. For both `ObjectGroup::Tables` and `ObjectGroup::Views`, obtain the relation ID from `load_catalog_page` and assert the four-component catalog shape.
4. Retain the existing unfiltered page assertions and three-component compatibility check for each kind.
5. Add a restrictive WHERE condition (`[id] >= 11`) and check the filtered last page, exact total of 15, and first visible ID of 15.
6. Attempt cleanup after the caught result: drop view, drop table, and drop schema if it exists; close the connection before reporting a cleanup error or resuming the original panic.
7. Describe three-component compatibility as previously accepted API input, without claiming a historical catalog format change.

### Task 2: Record the fix

**Files:**
- Modify: `CHANGELOG.md`

Add an Unreleased / Fixed entry linking PR #17 and issue #16 and describing restored SQL Server table and view DATA previews. Do not change the package version or create a release tag.

### Task 3: Validate the concrete patch

1. Run `cargo fmt --all -- --check` and `git diff --check`.
2. Run `cargo check --locked --all-features --test sqlserver_adapter`.
3. Run `cargo clippy --locked --all-targets --all-features -- -D warnings`.
4. Run `cargo test --locked --profile dev --all-targets --all-features` using the existing build cache. Distinguish environment-gated tests from live database execution.
5. Review the complete diff and commit only the listed implementation, test, changelog, and plan files, plus the confirmed CI prerequisite described below.

### Task 4: Update and merge PR #17

1. Verify contributor-branch push access before publishing. Although `maintainerCanModify` is true, both SSH and HTTPS dry-run pushes were denied by GitHub, so use an upstream companion PR without rewriting the contributor branch.
2. Push this branch and open a companion PR describing the regression coverage and cleanup. It initially includes PR #17's commits so CI validates the complete proposed change.
3. Approve PR #17's existing Actions run for its exact head and inspect its live SQL Server adapter and transaction tests. Require the companion PR's complete CI to pass, including the MongoDB prerequisite below; investigate failures without bypassing checks.
4. Merge the companion PR with a merge commit at its validated head, preserving the contributor's original commits. The original PR's head is then reachable from the default branch and GitHub can mark PR #17 as indirectly merged.
5. Verify both PR states, the original commits' ancestry, and automatic closure of issue #16 through the companion PR's `Fixes #16` reference.
6. Keep the original workspace's unrelated local commits intact when synchronizing the completed change.

### Confirmed CI prerequisite

PR #17's approved CI run fails before MongoDB tests start: MongoDB 8.0 exits with fatal ID 12257600 on the upgraded Azure kernel. The GitHub runner team confirms the incompatibility and recommends Ubuntu 22.04 in `actions/runner-images#15005`. Change only the MongoDB job's `runs-on` in `.github/workflows/ci.yml` to `ubuntu-22.04`, preserving MongoDB 8.0 and all test commands. Validate the workflow and require the companion PR's complete CI to pass; retain the original run's infrastructure failure in the validation record.
