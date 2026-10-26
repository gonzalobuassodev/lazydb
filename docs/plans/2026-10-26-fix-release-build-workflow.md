# Release Build Workflow Fix Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix the v0.2.2 Release workflow so pinned Linux images exist before dependency checks and cold release builds have enough time to finish.

**Architecture:** Keep the existing pinned Docker image and recovery workflow. Prepare each native Linux image before running commands inside it, raise Cargo build limits to 30 minutes, and give build jobs enough room for image preparation and packaging. Add distribution contract coverage for step order and timeout values, then dispatch the existing-tag recovery workflow after the fix reaches main.

**Tech Stack:** GitHub Actions YAML, POSIX shell, Python unittest, GitHub CLI.

---

### Task 1: Add release workflow regression coverage

**Files:**
- Modify: `scripts/release/test-build-linux.py`

**Steps:**
1. Add a test asserting Linux image preparation precedes dependency checks in `.github/workflows/release.yml`.
2. Assert the release build job timeout is at least 45 minutes and macOS/Windows Cargo builds use a 1800-second timeout.
3. Assert the Linux build wrapper uses a 1800-second Cargo timeout for both builds and Cargo commands run in the pinned image.
4. Run `python3 -B scripts/release/test-build-linux.py`; confirm it fails against the current workflow.

### Task 2: Fix release image ordering and build timeouts

**Files:**
- Modify: `.github/workflows/release.yml`
- Modify: `scripts/release/build-linux.sh`

**Steps:**
1. Move pinned Linux image preparation before dependency checks.
2. Raise release build job timeout to 45 minutes.
3. Raise Linux, macOS, and Windows release Cargo build limits from 600 to 1800 seconds.
4. Re-run `python3 -B scripts/release/test-build-linux.py` and `sh scripts/release/test-distribution.sh`.

### Task 3: Publish the workflow fix and recover v0.2.2

**Steps:**
1. Review the exact diff and run workflow validation checks.
2. Commit the focused fix and push it to `origin/main`.
3. Wait for CI on the fix commit to succeed.
4. Dispatch the Release workflow from `main` with existing tag `v0.2.2`; do not create or move a tag.
5. Monitor the exact recovery run until package artifacts and the GitHub Release are published, then monitor Pages and online installation checks.
