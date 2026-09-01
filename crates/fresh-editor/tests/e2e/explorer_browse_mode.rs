//! E2E coverage for the explorer's browse mode.
//!
//! Browse mode is what a bare `fresh` launch enters. It keeps the ordinary
//! tree — Right/Left still expand and collapse in place, compact chains still
//! fold — and adds three things on top:
//!
//!   * a `..` row, so you can leave the project root;
//!   * Enter as a *move* rather than an unfold: it re-roots the tree at the
//!     selected directory, so the view becomes `..` plus that directory's own
//!     contents;
//!   * files that open only on a deliberate gesture, read-only, reusing one
//!     viewer tab instead of accumulating one per glance.
//!
//! Each of those is a behaviour a future refactor could silently undo, and
//! each has a specific way of going wrong that these tests pin:
//!
//!   * `..` is a directory whose path is its *parent*, so the hidden-file
//!     filter (which judges a path by its own last component) will eat it
//!     inside any dot-directory unless explicitly exempted;
//!   * Enter and Right share a selection but must not share an effect;
//!   * `file_explorer_preview_selected` runs from all four navigate handlers
//!     and opens whatever the cursor lands on, so arrowing past a large file
//!     loads it — the reason cursoring through a directory used to stall.
//!
//! Assertions are on rendered output rather than internal state, per
//! CONTRIBUTING §2.

use crate::common::harness::EditorTestHarness;
use crossterm::event::{KeyCode, KeyModifiers};
use std::fs;

/// A project with a foldable chain, a plain directory, and two files:
///
/// ```text
/// <root>/
///   chain/a/b/c/leaf.txt   ← single-child chain, folds to `chain/a/b/c`
///   plain/inner.txt
///   alpha.txt
///   beta.txt
/// ```
fn browse_harness() -> EditorTestHarness {
    let mut harness = EditorTestHarness::with_temp_project(120, 30).unwrap();
    let root = harness.project_dir().unwrap();
    fs::create_dir_all(root.join("chain/a/b/c")).unwrap();
    fs::write(root.join("chain/a/b/c/leaf.txt"), "leaf").unwrap();
    fs::create_dir_all(root.join("plain")).unwrap();
    fs::write(root.join("plain/inner.txt"), "inner").unwrap();
    fs::write(root.join("alpha.txt"), "alpha contents").unwrap();
    fs::write(root.join("beta.txt"), "beta contents").unwrap();

    harness.editor_mut().enable_explorer_list_mode();
    harness
        .wait_until(|h| h.screen_to_string().contains(".."))
        .unwrap();
    harness.wait_for_file_explorer_item("chain").unwrap();
    harness
}

/// Move the explorer cursor down `n` times, settling async work each step so
/// a row that arrives late does not shift the selection underneath us.
fn down(harness: &mut EditorTestHarness, n: usize) {
    for _ in 0..n {
        harness.send_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        harness.process_async_and_render().unwrap();
    }
}

/// Browse mode opens the explorer and offers a way out of the project root.
#[test]
fn browse_mode_shows_a_parent_row() {
    let mut harness = browse_harness();
    let screen = harness.screen_to_string();

    assert!(
        screen.contains("File Explorer"),
        "browse mode must show the explorer, got:\n{screen}"
    );
    assert!(
        screen.lines().any(|l| l.contains("..")),
        "browse mode must offer a `..` row, got:\n{screen}"
    );
}

/// Right expands in place: the chain folds to one breadcrumb row and the rest
/// of the project stays on screen. This is the half of the behaviour that must
/// NOT change when Enter re-roots.
#[test]
fn right_arrow_expands_in_place_without_re_rooting() {
    let mut harness = browse_harness();
    down(&mut harness, 2); // root row, `..`, then `chain`

    harness
        .send_key(KeyCode::Right, KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.screen_to_string().contains("chain/a/b/c"))
        .unwrap();

    let screen = harness.screen_to_string();
    assert!(
        screen.contains("leaf.txt"),
        "expanding should reveal the chain's contents, got:\n{screen}"
    );
    // Still inside the project: the siblings are right where they were.
    assert!(
        screen.contains("plain") && screen.contains("alpha.txt"),
        "Right must expand in place, not re-root — siblings should remain, got:\n{screen}"
    );
}

/// Enter on a directory *enters* it: the view becomes that directory's own
/// contents plus `..`, and the siblings left behind are gone.
#[test]
fn enter_on_a_directory_re_roots_into_it() {
    let mut harness = browse_harness();
    down(&mut harness, 3); // root row, `..`, `chain`, then `plain`

    harness
        .send_key(KeyCode::Enter, KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.screen_to_string().contains("inner.txt"))
        .unwrap();

    let screen = harness.screen_to_string();
    assert!(
        screen.lines().any(|l| l.contains("..")),
        "the entered directory still needs a way back out, got:\n{screen}"
    );
    // The siblings of `plain` are no longer reachable from this view — that is
    // what distinguishes entering from expanding.
    assert!(
        !screen.contains("alpha.txt") && !screen.contains("chain"),
        "entering must replace the view, not extend it, got:\n{screen}"
    );
}

/// Enter on `..` walks back out to the parent.
#[test]
fn enter_on_parent_row_walks_back_out() {
    let mut harness = browse_harness();
    down(&mut harness, 3);
    harness
        .send_key(KeyCode::Enter, KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.screen_to_string().contains("inner.txt"))
        .unwrap();

    // Now inside `plain`. Cursor sits on the root row; `..` is the next row.
    down(&mut harness, 1);
    harness
        .send_key(KeyCode::Enter, KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.screen_to_string().contains("alpha.txt"))
        .unwrap();

    let screen = harness.screen_to_string();
    assert!(
        screen.contains("chain") && screen.contains("beta.txt"),
        "walking out should restore the parent's contents, got:\n{screen}"
    );
    // And we really left: `plain`'s contents are no longer on screen. Without
    // this the test passes vacuously whenever Enter merely expands, because the
    // parent's rows were never removed in the first place.
    assert!(
        !screen.contains("inner.txt"),
        "walking out must leave the entered directory behind, got:\n{screen}"
    );
}

/// Arrowing over a file must not open it.
///
/// The regression this pins is not cosmetic: preview-on-arrow makes the cost
/// of one keypress the cost of loading the file under it, so scrolling past a
/// large file stalls the browser.
#[test]
fn arrowing_over_files_opens_nothing() {
    let mut harness = browse_harness();
    // Walk the whole listing, files included.
    down(&mut harness, 5);

    let tabs = harness.get_tab_bar();
    assert!(
        !tabs.contains("alpha.txt") && !tabs.contains("beta.txt"),
        "moving the cursor must not open files, tab bar was:\n{tabs}"
    );
}

/// Enter on a file opens it read-only — the `[RO]` marker is the observable
/// half of `mark_buffer_read_only`, which sets the metadata flag the status
/// bar reads as well as the state flag the input gates read.
#[test]
fn enter_on_a_file_opens_it_read_only() {
    let mut harness = browse_harness();
    down(&mut harness, 4); // root, `..`, chain, plain, then alpha.txt

    harness
        .send_key(KeyCode::Enter, KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.get_tab_bar().contains("alpha.txt"))
        .unwrap();

    let status = harness.get_status_bar();
    assert!(
        status.contains("RO"),
        "a viewed file must be read-only, status bar was:\n{status}"
    );
}

/// Viewing a second file retires the first, so browsing a directory leaves one
/// tab rather than one per file looked at.
#[test]
fn viewing_a_second_file_retires_the_first() {
    let mut harness = browse_harness();
    down(&mut harness, 4); // alpha.txt
    harness
        .send_key(KeyCode::Enter, KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.get_tab_bar().contains("alpha.txt"))
        .unwrap();

    // Back to the tree, down one row to beta.txt, and view that instead.
    harness.editor_mut().focus_file_explorer();
    down(&mut harness, 1);
    harness
        .send_key(KeyCode::Enter, KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.get_tab_bar().contains("beta.txt"))
        .unwrap();

    let tabs = harness.get_tab_bar();
    assert!(
        !tabs.contains("alpha.txt"),
        "the previous viewer tab should have been retired, tab bar was:\n{tabs}"
    );
}

/// A single click on a directory expands it, the way Right does — the mouse
/// mirrors the keyboard rather than having a gesture of its own.
#[test]
fn single_click_on_a_directory_expands_it() {
    let mut harness = browse_harness();

    // Find the screen row holding `chain` and click it. Row math is read off
    // the rendered frame rather than computed, because sticky-ancestor rows
    // mean screen row is not `scroll_offset + n`.
    let row = harness
        .screen_to_string()
        .lines()
        .position(|l| l.contains("chain"))
        .expect("`chain` must be on screen") as u16;
    harness.mouse_click(6, row).unwrap();
    harness
        .wait_until(|h| h.screen_to_string().contains("chain/a/b/c"))
        .unwrap();

    let screen = harness.screen_to_string();
    assert!(
        screen.contains("leaf.txt"),
        "a single click should expand the directory, got:\n{screen}"
    );
    assert!(
        screen.contains("alpha.txt"),
        "a single click expands in place — it must not re-root, got:\n{screen}"
    );
}

/// A single click on a file selects it and nothing else. Same reasoning as the
/// arrow-key case: opening must stay behind a deliberate gesture.
#[test]
fn single_click_on_a_file_opens_nothing() {
    let mut harness = browse_harness();

    let row = harness
        .screen_to_string()
        .lines()
        .position(|l| l.contains("alpha.txt"))
        .expect("`alpha.txt` must be on screen") as u16;
    harness.mouse_click(6, row).unwrap();

    let tabs = harness.get_tab_bar();
    assert!(
        !tabs.contains("alpha.txt"),
        "a single click must not open the file, tab bar was:\n{tabs}"
    );
}
