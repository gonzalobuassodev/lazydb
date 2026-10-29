# README Restructure Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the README a concise project entry point with a clear installation path and links to complete reference documentation.

**Architecture:** Keep the project introduction, first-run workflow, database overview, integrations, and essential links in the README. Move installation lifecycle details to a dedicated guide, preserve integration details in their existing references, and add a task-oriented documentation index. Update existing documentation checks to follow the new content locations.

**Tech Stack:** GitHub-flavored Markdown, existing Rust documentation tests, local Markdown link and structure checks.

---

## Task 1: Preserve detailed installation instructions

**Files:**
- Create: `docs/installation.md`
- Modify: `docs/configuration.md`

1. Document Unix, Windows, and Homebrew installation, optional database runtimes, release archives, and source builds.
2. Preserve installer troubleshooting, MCP onboarding, update ownership, uninstall data retention, and purge boundaries.
3. Link configuration readers to the new lifecycle guide and include Valkey in the connection category overview.

## Task 2: Make the README an onboarding page

**Files:**
- Modify: `README.md`

1. Replace HTML spacing with a Markdown title, short introduction, three core capabilities, navigation links, and an accurate beta status note.
2. Place the main screenshot directly after the title, short description, and navigation links so readers see the application early. Follow the preview and core capabilities with installation, an explicit `lazydb` launch command, and first-connection/help instructions.
3. Keep one main preview and preserve the other five images in a collapsed gallery.
4. List all eleven database drivers, note Oracle and DuckDB runtime prerequisites, and link to capability details.
5. Reduce MCP, Neovim, LSP, configuration, contribution, and license content to actionable entry points.

## Task 3: Complete the destination documentation

**Files:**
- Create: `docs/README.md`
- Modify: `docs/database-capabilities.md`
- Modify: `docs/sql-language-server.md`
- Modify: `tests/docs.rs`

1. Add a task-oriented index covering installation, configuration, daily workflows, database guides, integrations, and contribution references.
2. Complete the driver overview and relocate shared limitations, runtime requirements, and database-specific support notes from the old README.
3. Preserve the Neovim plugin example and current LSP usage/completion behavior in the language-server guide, keeping implementation notes distinct.
4. Adjust existing documentation checks so the README links to detailed behavior instead of duplicating it; remove stale clipboard prose assertions.

## Task 4: Validate the documentation change

**Files:** All files above.

1. Run `git diff --check`; expect no whitespace errors.
2. Check relative links and heading anchors in changed Markdown files, including links nested inside HTML details; expect no unresolved targets.
3. Check heading nesting, fenced code blocks, image preservation, README length, and driver coverage against the existing driver registry.
4. Run the existing documentation tests with `cargo test --locked --test docs`; expect all tests to pass. If compiling the complete dependency graph is unnecessary, run the same dependency-free test source directly with `rustc --edition 2024 --test tests/docs.rs` and execute the resulting temporary binary.
5. Review the final diff for missing moved content and unrelated changes.

No runtime behavior, dependencies, Git commits, or remote changes are part of this task.

## Verification Results

- README reduced from 594 to 151 lines; the main preview image is on line 8, directly after the introduction and navigation links.
- All six original screenshot URLs are preserved, with five inside a collapsed gallery.
- Local checks passed for 106 relative links, 28 heading anchors, Markdown heading/fence structure, and HTML gallery nesting.
- All eleven registered drivers appear in the README overview, driver matrix, and catalog/version reference.
- Both existing documentation tests passed when compiled directly with `rustc --edition 2024 --test tests/docs.rs` and executed from a temporary directory.
- `rustfmt --edition 2024 --check tests/docs.rs` and `git diff --check` passed.
