impl ArborWindow {
    fn active_terminal(&self) -> Option<&TerminalSession> {
        let worktree_path = self.selected_worktree_path()?;
        let session_id = self.active_terminal_id_for_worktree(worktree_path)?;

        self.terminals.iter().find(|session| {
            session.id == session_id && session.worktree_path.as_path() == worktree_path
        })
    }

    fn write_input_to_terminal(&mut self, session_id: u64, input: &[u8]) -> Result<(), String> {
        if input.is_empty() {
            return Ok(());
        }

        let Some(index) = self
            .terminals
            .iter()
            .position(|session| session.id == session_id)
        else {
            return Ok(());
        };

        let Some(runtime) = self.terminals[index].runtime.clone() else {
            self.terminals[index].queued_input.extend_from_slice(input);
            return Ok(());
        };

        {
            let session = &self.terminals[index];
            runtime.write_input(session, input)?;
        }

        self.terminals[index].updated_at_unix_ms = current_unix_timestamp_millis();
        Ok(())
    }

    fn flush_queued_input_for_terminal(&mut self, session_id: u64) -> Result<(), String> {
        let Some(index) = self
            .terminals
            .iter()
            .position(|session| session.id == session_id)
        else {
            return Ok(());
        };
        let Some(runtime) = self.terminals[index].runtime.clone() else {
            return Ok(());
        };
        if self.terminals[index].queued_input.is_empty() {
            return Ok(());
        }

        let queued_input = std::mem::take(&mut self.terminals[index].queued_input);
        let session = self.terminals[index].clone();
        runtime.write_input(&session, &queued_input)?;
        self.terminals[index].updated_at_unix_ms = current_unix_timestamp_millis();
        Ok(())
    }

    fn clear_terminal_selection(&mut self) {
        self.terminal_selection = None;
        self.terminal_selection_drag_anchor = None;
    }

    fn clear_terminal_selection_for_session(&mut self, session_id: u64) {
        if self
            .terminal_selection
            .as_ref()
            .is_some_and(|selection| selection.session_id == session_id)
        {
            self.clear_terminal_selection();
        }
    }

    fn terminal_display_lines_for_session(&self, session_id: u64) -> Vec<String> {
        let Some(session) = self
            .terminals
            .iter()
            .find(|session| session.id == session_id)
        else {
            return vec![String::new()];
        };

        terminal_display_lines(session)
    }

    fn terminal_selection_for_session(&self, session_id: u64) -> Option<&TerminalSelection> {
        self.terminal_selection
            .as_ref()
            .filter(|selection| selection.session_id == session_id)
    }

    fn handle_terminal_output_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }

        let Some(session_id) = self.active_terminal_id_for_selected_worktree() else {
            return;
        };

        let lines = self.terminal_display_lines_for_session(session_id);
        let line_height = terminal_line_height_px(cx);
        let cell_width = terminal_cell_width_px(cx);
        let point = terminal_grid_position_from_pointer(
            event.position,
            self.terminal_scroll_handle.bounds(),
            self.terminal_scroll_handle.offset(),
            line_height,
            cell_width,
            lines.len(),
        );

        let Some(point) = point else {
            return;
        };

        if event.click_count >= 3 {
            if let Some((start, end)) = terminal_line_bounds(&lines, point) {
                self.terminal_selection = Some(TerminalSelection {
                    session_id,
                    anchor: start,
                    head: end,
                });
            } else {
                self.clear_terminal_selection_for_session(session_id);
            }
            self.terminal_selection_drag_anchor = None;
        } else if event.click_count == 2 {
            if let Some((start, end)) = terminal_token_bounds(&lines, point) {
                self.terminal_selection = Some(TerminalSelection {
                    session_id,
                    anchor: start,
                    head: end,
                });
            } else {
                self.clear_terminal_selection_for_session(session_id);
            }
            self.terminal_selection_drag_anchor = None;
        } else {
            self.terminal_selection = Some(TerminalSelection {
                session_id,
                anchor: point,
                head: point,
            });
            self.terminal_selection_drag_anchor = Some(point);
        }

        window.focus(&self.terminal_focus);
        self.focus_terminal_on_next_render = false;
        cx.notify();
    }

    fn handle_terminal_output_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }

        let Some(session_id) = self.active_terminal_id_for_selected_worktree() else {
            return;
        };
        let Some(anchor) = self.terminal_selection_drag_anchor else {
            return;
        };

        let lines = self.terminal_display_lines_for_session(session_id);
        let line_height = terminal_line_height_px(cx);
        let cell_width = terminal_cell_width_px(cx);
        let Some(head) = terminal_grid_position_from_pointer(
            event.position,
            self.terminal_scroll_handle.bounds(),
            self.terminal_scroll_handle.offset(),
            line_height,
            cell_width,
            lines.len(),
        ) else {
            return;
        };

        self.terminal_selection = Some(TerminalSelection {
            session_id,
            anchor,
            head,
        });
        cx.notify();
    }

    fn handle_terminal_output_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        if event.button == MouseButton::Left {
            self.terminal_selection_drag_anchor = None;
        }
    }

    fn track_terminal_command_input(&mut self, session_id: u64, keystroke: &Keystroke) {
        let Some(session) = self
            .terminals
            .iter_mut()
            .find(|session| session.id == session_id)
        else {
            return;
        };

        track_terminal_command_keystroke(session, keystroke);
    }

    fn copy_terminal_content_to_clipboard(&mut self, session_id: u64, cx: &mut Context<Self>) {
        let Some(session) = self
            .terminals
            .iter()
            .find(|session| session.id == session_id)
        else {
            return;
        };

        let clipboard_text =
            if let Some(selection) = self.terminal_selection_for_session(session_id) {
                let selected = terminal_selection_text(
                    &self.terminal_display_lines_for_session(session_id),
                    selection,
                );
                if !selected.is_empty() {
                    selected
                } else if !session.pending_command.trim().is_empty() {
                    session.pending_command.clone()
                } else {
                    session.output.clone()
                }
            } else if !session.pending_command.trim().is_empty() {
                session.pending_command.clone()
            } else {
                session.output.clone()
            };
        if clipboard_text.is_empty() {
            return;
        }

        cx.write_to_clipboard(ClipboardItem::new_string(clipboard_text));
    }

    fn append_pasted_text_to_pending_command(&mut self, session_id: u64, text: &str) {
        let Some(session) = self
            .terminals
            .iter_mut()
            .find(|session| session.id == session_id)
        else {
            return;
        };

        session.pending_command.push_str(text);
    }

    fn paste_clipboard_into_terminal(&mut self, session_id: u64, cx: &mut Context<Self>) {
        let Some(clipboard_item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = clipboard_item.text() else {
            return;
        };
        if text.is_empty() {
            return;
        }

        self.append_pasted_text_to_pending_command(session_id, &text);
        if let Err(error) = self.write_input_to_terminal(session_id, text.as_bytes()) {
            self.notice = Some(format!("failed to paste into terminal: {error}"));
        }
    }

    fn handle_terminal_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.quit_overlay_until.is_some() {
            // The quit overlay is modal — suppress terminal input entirely so
            // the key event propagates up to handle_global_key_down which
            // handles Enter/Escape for the overlay.
            return;
        }

        if self.command_palette_modal.is_some() {
            if event.keystroke.key.as_str() == "escape" {
                self.close_command_palette(cx);
                cx.stop_propagation();
                return;
            }
            // Keep all other keys out of the terminal while the palette is open
            // so they continue to route through the global modal handler.
            return;
        }

        if self.right_pane_search_active
            || self.worktree_notes_active
            || self.create_modal.is_some()
            || self.settings_modal.is_some()
            || self.github_auth_modal.is_some()
            || self.delete_modal.is_some()
            || self.manage_hosts_modal.is_some()
            || self.manage_presets_modal.is_some()
            || self.manage_repo_presets_modal.is_some()
            || self.daemon_auth_modal.is_some()
            || self.start_daemon_modal
            || self.connect_to_host_modal.is_some()
            || self.show_theme_picker
        {
            return;
        }

        let active_tab = self.active_center_tab_for_selected_worktree();

        // Handle file view editing before terminal input
        if matches!(active_tab, Some(CenterTab::FileView(_))) {
            if self.handle_file_view_key_down(event, cx) {
                cx.stop_propagation();
            }
            return;
        }

        let Some(CenterTab::Terminal(active_terminal_id)) = active_tab else {
            return;
        };

        if let Some(command) = terminal_keys::platform_command_for_keystroke(&event.keystroke) {
            match command {
                terminal_keys::TerminalPlatformCommand::Copy => {
                    self.copy_terminal_content_to_clipboard(active_terminal_id, cx);
                },
                terminal_keys::TerminalPlatformCommand::Paste => {
                    self.paste_clipboard_into_terminal(active_terminal_id, cx);
                },
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }

        self.clear_terminal_selection_for_session(active_terminal_id);

        let terminal_modes = self
            .terminals
            .iter()
            .find(|session| session.id == active_terminal_id)
            .map(|session| session.modes)
            .unwrap_or_default();

        let Some(input) =
            terminal_keys::terminal_bytes_from_keystroke(&event.keystroke, terminal_modes)
        else {
            // No bytes for this key — let the event propagate to the IME /
            // InputHandler so composed characters arrive via
            // `replace_text_in_range`.
            return;
        };

        self.track_terminal_command_input(active_terminal_id, &event.keystroke);
        if let Err(error) = self.write_input_to_terminal(active_terminal_id, &input) {
            self.notice = Some(format!("failed to write to terminal: {error}"));
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn focus_terminal_panel(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.right_pane_search_active = false;
        window.focus(&self.terminal_focus);
        self.focus_terminal_on_next_render = false;
    }
}
