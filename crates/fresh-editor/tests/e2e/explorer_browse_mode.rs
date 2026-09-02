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

/// `H` in the viewer shows the traditional three-column dump: a dashed
/// address, sixteen bytes under a `00`..`0F` header, and the char column.
///
/// Asserted as whole rows rather than by fishing for `DE` somewhere on screen —
/// the columns only mean anything together, and a per-token assertion would
/// pass on a dump whose alignment had drifted.
#[test]
fn hex_view_renders_address_bytes_and_dump_columns() {
    let mut harness = EditorTestHarness::with_temp_project(120, 30).unwrap();
    let root = harness.project_dir().unwrap();
    let mut bytes = vec![0x7F, 0x45, 0x4C, 0x46, 0x02, 0x01, 0x01, 0x00];
    bytes.extend_from_slice(&[0x00; 8]);
    bytes.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    bytes.extend_from_slice(b"Hello");
    fs::write(root.join("sample.bin"), &bytes).unwrap();

    harness.editor_mut().enable_explorer_list_mode();
    harness.wait_for_file_explorer_item("sample.bin").unwrap();
    down(&mut harness, 2); // root row, `..`, then sample.bin
    harness
        .send_key(KeyCode::Enter, KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.get_tab_bar().contains("sample.bin"))
        .unwrap();

    harness
        .send_key(KeyCode::Char('h'), KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.screen_to_string().contains("0000-0000"))
        .unwrap();

    let screen = harness.screen_to_string();

    let header = screen
        .lines()
        .find(|l| l.contains("Address"))
        .unwrap_or_else(|| panic!("no header row, got:\n{screen}"));
    assert!(
        header.contains("00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F"),
        "header must label all sixteen byte columns, got:\n{header}"
    );
    assert!(
        header.contains("DUMP"),
        "header must name the dump column, got:\n{header}"
    );

    let first = screen
        .lines()
        .find(|l| l.contains("0000-0000"))
        .unwrap_or_else(|| panic!("no first data row, got:\n{screen}"));
    assert!(
        first.contains("7F 45 4C 46 02 01 01 00"),
        "first row must show the file's opening bytes, got:\n{first}"
    );
    assert!(
        first.contains(".ELF"),
        "the dump column must render printable bytes as characters, got:\n{first}"
    );

    // Second row proves the address advances by 0x10 and that a high byte
    // survives as hex while showing as `.` in the dump.
    let second = screen
        .lines()
        .find(|l| l.contains("0000-0010"))
        .unwrap_or_else(|| panic!("no second data row, got:\n{screen}"));
    assert!(
        second.contains("DE AD BE EF"),
        "high bytes must survive into the hex column, got:\n{second}"
    );
    assert!(
        second.contains("Hello"),
        "the dump column must keep tracking the same bytes, got:\n{second}"
    );
}

/// `H` toggles back to text, so the test above cannot pass vacuously on a view
/// that was never text to begin with.
#[test]
fn hex_view_toggles_back_to_text() {
    let mut harness = EditorTestHarness::with_temp_project(120, 30).unwrap();
    let root = harness.project_dir().unwrap();
    fs::write(root.join("plain.txt"), "readable text here").unwrap();

    harness.editor_mut().enable_explorer_list_mode();
    harness.wait_for_file_explorer_item("plain.txt").unwrap();
    down(&mut harness, 2);
    harness
        .send_key(KeyCode::Enter, KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.get_tab_bar().contains("plain.txt"))
        .unwrap();

    harness
        .send_key(KeyCode::Char('h'), KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.screen_to_string().contains("0000-0000"))
        .unwrap();

    harness
        .send_key(KeyCode::Char('h'), KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| !h.screen_to_string().contains("0000-0000"))
        .unwrap();

    let screen = harness.screen_to_string();
    assert!(
        screen.contains("readable text here"),
        "toggling back must restore the text view, got:\n{screen}"
    );
}

/// A 4 KiB fixture whose every 16-byte row begins with its own row number, so
/// a rendered row identifies itself: row N is `N 52 4F 57` ("N R O W") at byte
/// N*16.
fn hex_probe_harness() -> EditorTestHarness {
    let mut harness = EditorTestHarness::with_temp_project(140, 30).unwrap();
    let root = harness.project_dir().unwrap();
    let mut data = Vec::new();
    for row in 0u16..256 {
        data.push(row as u8);
        data.extend_from_slice(b"ROW");
        data.extend_from_slice(&[0u8; 12]);
    }
    fs::write(root.join("probe.bin"), &data).unwrap();

    harness.editor_mut().enable_explorer_list_mode();
    harness.wait_for_file_explorer_item("probe.bin").unwrap();
    down(&mut harness, 2);
    harness
        .send_key(KeyCode::Enter, KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.get_tab_bar().contains("probe.bin"))
        .unwrap();
    harness
        .send_key(KeyCode::Char('h'), KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.screen_to_string().contains("0000-0000"))
        .unwrap();
    harness
}

/// The search options row carries a fourth checkbox. Asserted alongside the
/// existing three: the row's column math is duplicated between
/// `SearchOptionsLayout::compute` and the paint walk, and a `debug_assert_eq!`
/// compares them — so this test failing to even render is the signal that the
/// two drifted.
#[test]
fn search_options_row_offers_a_hex_checkbox() {
    let mut harness = hex_probe_harness();
    harness
        .send_key(KeyCode::Char('f'), KeyModifiers::CONTROL)
        .unwrap();
    harness
        .wait_until(|h| h.screen_to_string().contains("Case Sensitive"))
        .unwrap();

    let screen = harness.screen_to_string();
    let row = screen
        .lines()
        .find(|l| l.contains("Case Sensitive"))
        .unwrap_or_else(|| panic!("no options row, got:\n{screen}"));
    assert!(
        row.contains("Regex") && row.contains("Hex"),
        "the options row must offer Hex beside Regex, got:\n{row}"
    );
}

/// A hex byte pattern finds raw bytes and scrolls the dump to them.
///
/// The offset is what matters, so the assertion is on the rendered address row
/// rather than a match count — a count of 1 would pass even if the editor
/// jumped to the wrong place.
#[test]
fn hex_search_finds_a_byte_pattern_at_the_right_offset() {
    let mut harness = hex_probe_harness();
    harness
        .send_key(KeyCode::Char('f'), KeyModifiers::CONTROL)
        .unwrap();
    harness
        .wait_until(|h| h.screen_to_string().contains("Case Sensitive"))
        .unwrap();
    harness
        .send_key(KeyCode::Char('x'), KeyModifiers::ALT)
        .unwrap();

    // Row 0xC8 lives at byte 0xC8 * 16 = 0x0C80, far below the opening screen.
    for c in "C8 52 4F 57".chars() {
        harness
            .send_key(KeyCode::Char(c), KeyModifiers::NONE)
            .unwrap();
    }
    harness
        .send_key(KeyCode::Enter, KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.screen_to_string().contains("0000-0C80"))
        .unwrap();

    let screen = harness.screen_to_string();
    let hit = screen
        .lines()
        .find(|l| l.contains("0000-0C80"))
        .unwrap_or_else(|| panic!("dump did not scroll to the match, got:\n{screen}"));
    assert!(
        hit.contains("C8 52 4F 57"),
        "the match row must show the searched bytes, got:\n{hit}"
    );
    // Regression: the line-based scroll used to leave the dump blank.
    assert!(
        screen.contains("Address") && screen.contains("DUMP"),
        "the dump must still be rendered after a search, got:\n{screen}"
    );
}

/// Ctrl+G in hex mode is an address prompt, and jumping scrolls the dump.
#[test]
fn goto_address_jumps_the_dump_to_that_address() {
    let mut harness = hex_probe_harness();
    harness
        .send_key(KeyCode::Char('g'), KeyModifiers::CONTROL)
        .unwrap();
    harness
        .wait_until(|h| h.screen_to_string().contains("Go to address"))
        .unwrap();

    for c in "0000-0A00".chars() {
        harness
            .send_key(KeyCode::Char(c), KeyModifiers::NONE)
            .unwrap();
    }
    harness
        .send_key(KeyCode::Enter, KeyModifiers::NONE)
        .unwrap();
    harness
        .wait_until(|h| h.screen_to_string().contains("0000-0A00"))
        .unwrap();

    let screen = harness.screen_to_string();
    let hit = screen
        .lines()
        .find(|l| l.contains("0000-0A00"))
        .unwrap_or_else(|| panic!("dump did not scroll to the address, got:\n{screen}"));
    // 0x0A00 / 16 = row 0xA0, so the row's first byte is 0xA0.
    assert!(
        hit.contains("A0 52 4F 57"),
        "the address row must be the one holding that byte, got:\n{hit}"
    );
}
