//! The LIST.COM file viewer: open a file to *look* at, not to edit.
//!
//! Enter on a file in the browser lands here rather than in
//! `Editor::open_file`. Two differences: the buffer comes up read-only, and
//! only one viewer tab survives at a time — browsing is a sequence of glances,
//! and each glance would otherwise leave a tab behind.

use crate::app::Editor;
use crate::input::buffer_mode::BufferMode;
use crate::input::keybindings::{Action, KeyContext};
use crate::model::event::BufferId;
use crossterm::event::{KeyCode, KeyModifiers};
use fresh_i18n::t;
use std::path::Path;

/// Key-context mode name for a file open in the LIST viewer.
pub const VIEWER_MODE: &str = "viewer";

impl Editor {
    /// Install the viewer mode's keybindings and registry entry.
    ///
    /// Idempotent, and called on every viewer open rather than once at startup:
    /// `KeybindingResolver::reload_from_config` rebuilds on any config, keymap
    /// or settings change and carries over only the plugin-default tier, so a
    /// mode registered once would quietly stop resolving after the user touched
    /// Settings. The help panel does the same thing for the same reason.
    pub(crate) fn ensure_viewer_mode_registered(&mut self) {
        {
            let mut kb = self.keybindings.write().unwrap();
            kb.clear_plugin_defaults_for_mode(VIEWER_MODE);
            // Motion, scrolling, copy and search all keep working — a viewer
            // that could not be navigated would be useless. Only the editing
            // actions are refused, and those are refused by the read-only flag
            // rather than by withholding bindings.
            kb.set_mode_inherits_normal_bindings(VIEWER_MODE, true);
            // `H` for the hex dump — the classic viewer key. Only unambiguous
            // because the viewer is read-only, so a bare letter is free.
            for key in [KeyCode::Char('h'), KeyCode::Char('H')] {
                kb.load_plugin_default(
                    KeyContext::Mode(VIEWER_MODE.to_string()),
                    key,
                    KeyModifiers::NONE,
                    Action::ToggleHexView,
                );
            }
            // The other half of the loop: Esc returns to the listing you
            // came from, so browse → view → browse never needs the mouse.
            kb.load_plugin_default(
                KeyContext::Mode(VIEWER_MODE.to_string()),
                KeyCode::Esc,
                KeyModifiers::NONE,
                Action::FocusFileExplorer,
            );
        }
        self.mode_registry.register(
            BufferMode::new(VIEWER_MODE)
                .with_read_only(true)
                .with_inherit_normal_bindings(true),
        );
    }

    /// Open `path` as a read-only view, the way `LIST <file>` does.
    ///
    /// Returns `Ok(())` on success; the caller reports failures, because the
    /// browser wants to stay on screen and put the error in the status bar
    /// rather than tear down.
    pub fn open_file_in_viewer(&mut self, path: &Path) -> anyhow::Result<()> {
        self.ensure_viewer_mode_registered();
        let buffer_id = self.open_file(path)?;

        // Enter is the deliberate gesture, so the tab is a real one rather than
        // a preview that the next arrow keypress would replace.
        self.active_window_mut()
            .promote_buffer_from_preview(buffer_id);

        {
            let window = self.active_window_mut();
            window.viewer_buffers.insert(buffer_id);
            // Sets `metadata.read_only` *and* `state.editing_disabled` together.
            // Setting only the latter — as several open paths do — leaves the
            // `[RO]` status segment dark and makes `Action::ToggleReadOnly` read
            // the wrong current value and toggle backwards.
            window.mark_buffer_read_only(buffer_id, true);
        }

        self.default_to_hex_if_binary();

        // Browsing is a sequence of glances, and without this each glance
        // leaves a tab behind — walk a directory of fifty files and you have
        // fifty tabs you never asked to keep. Retire the previous glance now
        // that this one has replaced it.
        //
        // Done *after* the new buffer exists, so closing the old one can never
        // empty the window and make the editor synthesize a placeholder.
        self.retire_previous_viewer_buffers(buffer_id);

        // Enter puts you *in* the file, the way LIST does — otherwise the
        // browser would still own the keyboard. `Esc` hands it back.
        self.active_window_mut().focus_editor();

        self.set_status_message(
            t!(
                "viewer.opened",
                name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            )
            .to_string(),
        );
        Ok(())
    }

    /// Close viewer tabs opened before this one, so browsing leaves at most one
    /// behind.
    ///
    /// Only tabs this browser opened are touched — anything the user opened
    /// normally is not the browser's to close. And only while they are still
    /// *glances*: a file the user unlocked for editing, or modified, has
    /// stopped being disposable. Those are left open and dropped from tracking,
    /// so a later glance never reconsiders them.
    ///
    /// `close_buffer` refuses a modified buffer on its own, so the modified
    /// check here is belt-and-braces; the `read_only` check is the one doing
    /// real work, since unlocking a file is the gesture that says "I want to
    /// keep this".
    fn retire_previous_viewer_buffers(&mut self, keep: BufferId) {
        let previous: Vec<BufferId> = self
            .active_window()
            .viewer_buffers
            .iter()
            .copied()
            .filter(|&id| id != keep)
            .collect();

        for id in previous {
            let adopted = {
                let win = self.active_window();
                let still_read_only = win
                    .buffer_metadata
                    .get(&id)
                    .map(|meta| meta.read_only)
                    .unwrap_or(false);
                let modified = win
                    .buffers
                    .get(&id)
                    .is_some_and(|state| state.buffer.is_modified());
                !still_read_only || modified
            };

            if adopted || self.close_buffer(id).is_ok() {
                self.active_window_mut().viewer_buffers.remove(&id);
            }
        }
    }

    /// Put the active split into hex view when the buffer is binary.
    ///
    /// The alternative — the text renderer's `<7F><45><4C>` escape soup — is
    /// not a useful way to look at bytes, and anyone looking at a binary wants
    /// the dump. `H` toggles back to that escape rendering for the times it is
    /// wanted. Text files are untouched: defaulting a `.txt` to hex would be
    /// absurd.
    ///
    /// Shared by the read-only viewer and the explorer's preview so the two
    /// cannot disagree about what opening a binary looks like — they did, and
    /// a previewed binary came up as escape soup while an Entered one came up
    /// as a dump.
    pub(crate) fn default_to_hex_if_binary(&mut self) {
        if self.active_state().buffer.is_binary()
            && self.active_view_mode() != crate::state::ViewMode::Hex
        {
            self.active_window_mut().handle_toggle_hex_view();
        }
    }

    /// Whether the active buffer is a LIST viewer.
    pub fn active_buffer_is_viewer(&self) -> bool {
        let id = self.active_buffer();
        self.active_window().viewer_buffers.contains(&id)
    }
}
