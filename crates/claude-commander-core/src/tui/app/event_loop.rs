//! Main event loop: tick dispatch, event processing, and config hot-reload.

use super::*;

impl App {
    /// Main event loop
    pub(super) async fn main_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<()> {
        // Kick off an initial background preview fetch
        self.update_selection();
        self.spawn_preview_update();

        loop {
            // View-switch clearing happens inside `render` via the `Clear`
            // widget (see the `clear_right_pane` handling there). We must not
            // call `terminal.clear()`: since ratatui 0.30 it reads the cursor
            // position from stdin, which races our background input reader,
            // times out, and kills the loop.

            // Render with whatever data we have — never blocks on I/O
            terminal
                .draw(|f| self.render(f))
                .map_err(|e| TuiError::RenderError(e.to_string()))?;

            // Wait for at least one event
            let Some(event) = self.event_loop.next().await else {
                break;
            };

            // Process first event, then drain all pending events.
            // This ensures rapid keypresses are handled immediately
            // without waiting for the next render cycle.
            let mut needs_tick = false;
            needs_tick |= self.process_event(event).await;

            while let Some(event) = self.event_loop.try_next() {
                needs_tick |= self.process_event(event).await;
            }

            // Periodic background work (only on Tick). The PR-status,
            // project-pull, agent-state, and state-sync loops now run inside the
            // service (see `CommanderService::spawn_background_tasks`); the tick
            // only refreshes the rendered list and the preview pane.
            if needs_tick {
                self.refresh_list_items().await;

                // Spawn non-blocking preview update
                self.spawn_preview_update();
            }

            if self.ui_state.should_quit {
                break;
            }
        }

        Ok(())
    }

    /// Process a single event, returns true if it was a Tick
    pub(super) async fn process_event(&mut self, event: AppEvent) -> bool {
        match event {
            AppEvent::Input(input) => {
                let old_session = self.ui_state.selected_session_id;
                let old_project = self.ui_state.selected_project_id;

                self.handle_input(input).await;
                // Keep selection IDs in sync after input (needed for
                // correct behavior when draining multiple events)
                self.update_selection();

                // Immediately fetch preview when selection changes
                if self.ui_state.selected_session_id != old_session
                    || self.ui_state.selected_project_id != old_project
                {
                    // Cancel any in-flight fetch for the old selection
                    self.ui_state.preview_update_spawned_at = None;
                    self.spawn_preview_update();
                }
            }
            AppEvent::StateUpdate(update) => self.handle_state_update(update).await,
            AppEvent::Tick => {
                self.ui_state.tick_count = self.ui_state.tick_count.wrapping_add(1);
                if self.ui_state.tick_count.is_multiple_of(3) {
                    self.ui_state.throbber_state.calc_next();
                }

                // Resolve pending digit jump if debounce window expired
                if let Some(crate::tui::digit_accumulator::DigitResult::Jump(n)) =
                    self.digit_accumulator.tick()
                {
                    self.jump_to_session_number(n);
                }

                // Check for config file changes roughly once per second
                // (tick_count wraps at u64::MAX, is_multiple_of(30) at 30fps ≈ 1s)
                if self.ui_state.tick_count.is_multiple_of(30) {
                    self.check_config_reload();
                }
                return true;
            }
            AppEvent::Quit => {
                self.ui_state.should_quit = true;
            }
        }
        false
    }

    /// Check if `config.toml` has been modified externally and refresh the local cache.
    pub(super) fn check_config_reload(&mut self) {
        match self.service.reload_config() {
            Ok(true) => {
                debug!("Config hot-reloaded from disk");
                let old_servers = self.config.remote_servers.clone();
                self.config = self.service.read_config();
                let base = self
                    .config
                    .theme
                    .preset
                    .as_deref()
                    .and_then(Theme::from_preset)
                    .unwrap_or_default();
                self.theme = base.with_overrides(&self.config.theme);

                // Reconcile the live backends against the new remote-server list
                // (add/remove/rebuild handles) when it changed.
                let new_servers = self.config.remote_servers.clone();
                if old_servers != new_servers {
                    self.apply_remote_servers_reload(&old_servers, &new_servers);
                }
            }
            Ok(false) => {}
            Err(e) => {
                debug!("Config reload check failed: {}", e);
            }
        }
    }
}
