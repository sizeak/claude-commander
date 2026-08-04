//! Kanban board widget rendering.
//!
//! [`BoardWidget`] renders a [`Board`] into a rectangle: a project sidebar on
//! the left, thin vertical separators, and one full-height lane per section
//! column. Each lane is a header line plus a vertical stack of project-coloured
//! bordered cards, **one session per card**. A card's border title is its
//! session number and title; its single interior line carries the status glyph,
//! row markers, PR pill / `[branch]`, optional `(program)` suffix, and — right
//! aligned — three clickable action buttons (`[>_]` shell, `[±]` review diff,
//! `[i]` info). Stacked children render as their own cards, indented and
//! narrowed beneath their base.
//!
//! Unlike a plain [`StatefulWidget`](ratatui::widgets::StatefulWidget), the
//! render method returns [`BoardRenderOutput`] — the per-row hit regions, the
//! per-button hit regions, and the resolved [`BoardRects`] — so the app can map
//! mouse clicks and wheel scrolls back to board positions/actions. This mirrors
//! how the status bar returns its clickable `ActionButton`s rather than stashing
//! them in widget state.
//!
//! ## Scroll and clipping
//!
//! Each column scrolls independently in display lines (stored in
//! [`BoardState::scroll`]). Cards at the top/bottom edges of a lane's viewport
//! are clipped to the visible region: a partially-scrolled card renders its
//! border at the clip line (so it reads as a smaller box) and only its visible
//! interior row is drawn. This is the simplest behaviour that never draws
//! outside the lane; whole-card-only scrolling is out of scope for v1.

use std::collections::{HashMap, HashSet};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Widget},
};

use crate::theme::Theme;
use crate::widgets::{pr_colors, status_glyph};
use claude_commander_core::git::BlockReason;
use claude_commander_core::session::{Board, BoardPos, ProjectId, SessionId, SessionListItem};

use super::layout::{self, BoardRects};
use super::state::BoardState;

use status_glyph::{COMMENT_MARKER, KEEP_ALIVE_MARKER, LFS_MARKER};

/// Horizontal shift (and width reduction) applied to a stacked child's card so
/// it reads as nested one level under the base card directly above it.
const CHILD_INDENT: u16 = 2;

/// The three per-card action buttons, right-aligned in the interior line. Each
/// is recorded as a [`BoardButtonRegion`] so a click both selects the card's
/// session and fires the corresponding command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardButton {
    /// `[>_]` — open the session's shell (`UserCommand::SelectShell`).
    Shell,
    /// `[±]` — open the review diff (`UserCommand::OpenReviewDiff`).
    Review,
    /// `[i]` — open the info modal (`UserCommand::OpenInfo`).
    Info,
}

impl CardButton {
    /// The bracketed label rendered for this button.
    fn label(self) -> &'static str {
        match self {
            CardButton::Shell => "[>_]",
            CardButton::Review => "[±]",
            CardButton::Info => "[i]",
        }
    }

    /// The buttons in left-to-right render order.
    const ORDER: [CardButton; 3] = [CardButton::Shell, CardButton::Review, CardButton::Info];
}

/// A clickable region on the board: the screen rectangle a row occupies paired
/// with the board position a click there selects. One is emitted per *visible*
/// session card and per visible sidebar (project) row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardHitRegion {
    pub rect: Rect,
    pub pos: BoardPos,
}

/// A clickable action button on a card: its screen rectangle, the board position
/// (card) it belongs to, and which action it fires. One is emitted per visible
/// button. The app checks these *before* the row [`BoardHitRegion`]s so a click
/// on a button both selects the card and dispatches the button's command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardButtonRegion {
    pub rect: Rect,
    pub pos: BoardPos,
    pub button: CardButton,
}

/// Interior padding inside a card's borders, applied symmetrically: content
/// starts one column in from the left border and the buttons end one column
/// short of the right border.
const CARD_PAD: u16 = 1;

/// What [`BoardWidget::render`] hands back to the app: the row hit regions and
/// button hit regions for click mapping, and the resolved column/sidebar
/// rectangles for wheel-scroll targeting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardRenderOutput {
    pub hit_regions: Vec<BoardHitRegion>,
    pub button_regions: Vec<BoardButtonRegion>,
    /// One region per visible sidebar server heading; a click opens that
    /// server's Settings → Programs tab (the whole heading row, including its
    /// `⚙` affordance, is the target).
    pub heading_regions: Vec<(Rect, claude_commander_core::backend::BackendId)>,
    pub rects: BoardRects,
}

/// Full-screen kanban board widget. Builder mirrors the old tree list: borrow a
/// [`Board`] and [`Theme`], then layer on tick/labels/selection/colours before
/// rendering.
pub struct BoardWidget<'a> {
    board: &'a Board,
    theme: &'a Theme,
    tick: u64,
    review_labels: &'a [String],
    invert_pr_label_color: bool,
    show_session_program: bool,
    comment_sessions: Option<&'a HashSet<SessionId>>,
    pull_blocked_projects: Option<&'a HashMap<ProjectId, BlockReason>>,
    project_colors: Option<&'a HashMap<ProjectId, (Color, Color)>>,
    /// Precomputed column-major session numbering (id → 1-based number), built
    /// once per state change by the app. `None` (used only in unit tests that
    /// don't care about numbers) renders every row's number as 0.
    session_numbers: Option<&'a HashMap<SessionId, usize>>,
    /// Whether the board spans more than one distinct program, precomputed by
    /// the app. Gates the `(program)` suffix together with `show_session_program`.
    mixed_programs: bool,
    selected: Option<BoardPos>,
    rounded: bool,
}

impl<'a> BoardWidget<'a> {
    /// Create a board widget over `board`, styled by `theme`.
    pub fn new(board: &'a Board, theme: &'a Theme) -> Self {
        Self {
            board,
            theme,
            tick: 0,
            review_labels: &[],
            invert_pr_label_color: false,
            show_session_program: true,
            comment_sessions: None,
            pull_blocked_projects: None,
            project_colors: None,
            session_numbers: None,
            mixed_programs: false,
            selected: None,
            rounded: false,
        }
    }

    /// Set the tick counter for spinner animation.
    pub fn tick(mut self, tick: u64) -> Self {
        self.tick = tick;
        self
    }

    /// Configure the labels that flag an open PR as awaiting reviewer action.
    pub fn review_labels(mut self, labels: &'a [String]) -> Self {
        self.review_labels = labels;
        self
    }

    /// When true, render PR labels as coloured text on the default bg
    /// (pre-pill behaviour). Default false renders them as a coloured pill.
    pub fn invert_pr_label_color(mut self, b: bool) -> Self {
        self.invert_pr_label_color = b;
        self
    }

    /// When false, never show the `(program)` suffix. When true (default), show
    /// it only if the board has more than one distinct program.
    pub fn show_session_program(mut self, b: bool) -> Self {
        self.show_session_program = b;
        self
    }

    /// Mark a set of sessions as having pending review comments (renders a `*`).
    pub fn comment_sessions(mut self, sessions: &'a HashSet<SessionId>) -> Self {
        self.comment_sessions = Some(sessions);
        self
    }

    /// Mark projects whose most recent auto-pull was held back (renders a `⚠` on
    /// the card border title and the sidebar entry). Borrows the app's
    /// `project_pull_blocked` map directly — only membership matters here, so
    /// the block reason is never inspected. The board model deliberately doesn't
    /// carry pull-blocked state, so the app supplies it at render time.
    pub fn pull_blocked_projects(mut self, blocked: &'a HashMap<ProjectId, BlockReason>) -> Self {
        self.pull_blocked_projects = Some(blocked);
        self
    }

    /// Per-project (border, session-title) colours. The app builds this from the
    /// name-sorted project order via [`Theme::project_color`].
    pub fn project_colors(mut self, colors: &'a HashMap<ProjectId, (Color, Color)>) -> Self {
        self.project_colors = Some(colors);
        self
    }

    /// Column-major session numbering (id → 1-based number), precomputed once
    /// per state change by the app rather than rebuilt per frame.
    pub fn session_numbers(mut self, numbers: &'a HashMap<SessionId, usize>) -> Self {
        self.session_numbers = Some(numbers);
        self
    }

    /// Whether the board spans more than one distinct program (precomputed by
    /// the app). Together with `show_session_program`, gates the `(program)`
    /// suffix on session rows.
    pub fn mixed_programs(mut self, b: bool) -> Self {
        self.mixed_programs = b;
        self
    }

    /// Set the current cursor position.
    pub fn selected(mut self, selected: Option<BoardPos>) -> Self {
        self.selected = selected;
        self
    }

    /// Use rounded card borders (mirrors `config.rounded_borders`).
    pub fn rounded(mut self, b: bool) -> Self {
        self.rounded = b;
        self
    }

    fn session_has_comments(&self, id: &SessionId) -> bool {
        self.comment_sessions.is_some_and(|s| s.contains(id))
    }

    fn project_is_pull_blocked(&self, id: &ProjectId) -> bool {
        self.pull_blocked_projects
            .is_some_and(|m| m.contains_key(id))
    }

    /// (border colour, session-title colour) for a project, falling back to the
    /// primary text colour when the map is missing (it never is in practice —
    /// the app always builds a full map).
    fn project_color(&self, id: ProjectId) -> (Color, Color) {
        self.project_colors
            .and_then(|m| m.get(&id).copied())
            .unwrap_or((self.theme.text_primary, self.theme.text_primary))
    }

    /// Render the board into `area`, updating `state`'s per-column scroll and
    /// returning the hit regions and resolved rectangles.
    pub fn render(self, area: Rect, buf: &mut Buffer, state: &mut BoardState) -> BoardRenderOutput {
        let mut hit_regions = Vec::new();
        let mut button_regions = Vec::new();
        let mut heading_regions = Vec::new();
        let n_cols = self.board.columns.len();
        let rects = layout::column_rects(area, n_cols);

        if area.width == 0 || area.height == 0 {
            return BoardRenderOutput {
                hit_regions,
                button_regions,
                heading_regions,
                rects,
            };
        }

        // Defensive: one scroll slot per addressable column (sidebar + columns).
        // `BoardState::sync` normally sizes this, but a render before the first
        // sync must not panic.
        if state.scroll.len() < n_cols + 1 {
            state.scroll.resize(n_cols + 1, 0);
        }

        let numbers = self.session_numbers;
        let show_program = self.show_session_program && self.mixed_programs;
        let selected = self.selected;

        // Thin vertical separators (sidebar/column and column/column dividers).
        let sep_style = Style::default().fg(self.theme.border_unfocused);
        let right = area.x.saturating_add(area.width);
        for &sx in &rects.separators {
            if sx < right {
                for y in area.y..area.y.saturating_add(area.height) {
                    buf[(sx, y)].set_symbol("│").set_style(sep_style);
                }
            }
        }

        self.render_sidebar(
            rects.sidebar,
            buf,
            state,
            selected,
            &mut hit_regions,
            &mut heading_regions,
        );

        for (i, column) in self.board.columns.iter().enumerate() {
            let col_rect = rects.columns[i];
            self.render_column(
                i,
                column,
                col_rect,
                buf,
                state,
                selected,
                numbers,
                show_program,
                &mut hit_regions,
                &mut button_regions,
            );
        }

        BoardRenderOutput {
            hit_regions,
            button_regions,
            heading_regions,
            rects,
        }
    }

    /// Render the project sidebar (addressable column 0).
    fn render_sidebar(
        &self,
        rect: Rect,
        buf: &mut Buffer,
        state: &mut BoardState,
        selected: Option<BoardPos>,
        hit_regions: &mut Vec<BoardHitRegion>,
        heading_regions: &mut Vec<(Rect, claude_commander_core::backend::BackendId)>,
    ) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }

        // Display lines interleave per-server heading rows (only when more
        // than one backend is configured — a lone local server is suppressed
        // so single-machine boards look unchanged) with the selectable project
        // rows. Selection/hit-regions address project rows only; headings are
        // informational.
        enum SideLine {
            Header(usize),
            Project(usize),
        }
        let show_headers = self.board.servers.len() > 1;
        let mut display: Vec<SideLine> = Vec::new();
        let mut line_of_project: Vec<usize> = vec![0; self.board.projects.len()];
        if show_headers {
            for (si, server) in self.board.servers.iter().enumerate() {
                display.push(SideLine::Header(si));
                for idx in server.projects.clone() {
                    line_of_project[idx] = display.len();
                    display.push(SideLine::Project(idx));
                }
            }
        } else {
            for (idx, slot) in line_of_project.iter_mut().enumerate() {
                *slot = display.len();
                display.push(SideLine::Project(idx));
            }
        }

        let height = rect.height as usize;
        let total = display.len();

        // Keep the selected project's display line visible, then clamp so we
        // never scroll past the last screenful.
        let sel_row = selected.filter(|p| p.col == 0).map(|p| p.row);
        let sel_line = sel_row.and_then(|r| line_of_project.get(r).copied());
        let max_scroll = total.saturating_sub(height);
        let mut sc = state.scroll[0];
        if let Some(l) = sel_line {
            // When the selected project sits directly under its server heading,
            // scroll the heading into view with it so the group context stays
            // visible.
            let top = if l > 0 && matches!(display[l - 1], SideLine::Header(_)) {
                l - 1
            } else {
                l
            };
            if top < sc {
                sc = top;
            } else if l >= sc + height {
                sc = l + 1 - height;
            }
        }
        sc = sc.min(max_scroll);
        state.scroll[0] = sc;

        let sel_style = self.theme.selection().add_modifier(Modifier::BOLD);
        for row in 0..height {
            let li = sc + row;
            let Some(line) = display.get(li) else { break };
            let y = rect.y + row as u16;
            match line {
                SideLine::Header(si) => {
                    let server = &self.board.servers[*si];
                    self.render_server_heading(server, rect, y, buf);
                    heading_regions.push((
                        Rect {
                            x: rect.x,
                            y,
                            width: rect.width,
                            height: 1,
                        },
                        server.backend,
                    ));
                }
                SideLine::Project(idx) => {
                    let idx = *idx;
                    let entry = &self.board.projects[idx];
                    let (proj_color, _) = self.project_color(entry.project_id);
                    let blocked = self.project_is_pull_blocked(&entry.project_id);
                    // Indent project rows one cell under their server heading.
                    let indent: u16 = if show_headers { 1 } else { 0 };

                    let count_str = entry.session_count.to_string();
                    let count_w = count_str.chars().count() as u16;
                    let prefix_w: u16 = if blocked { 2 } else { 0 };
                    // Reserve a gap plus the count on the right for the name.
                    let name_budget = rect.width.saturating_sub(indent + prefix_w + count_w + 1);

                    let mut left_spans: Vec<Span<'static>> = Vec::new();
                    if blocked {
                        left_spans.push(Span::styled(
                            "⚠ ",
                            Style::default().fg(self.theme.agent_waiting),
                        ));
                    }
                    left_spans.push(Span::styled(
                        entry.name.clone(),
                        Style::default().fg(proj_color).add_modifier(Modifier::BOLD),
                    ));
                    buf.set_line(
                        rect.x + indent,
                        y,
                        &Line::from(left_spans),
                        prefix_w + name_budget,
                    );

                    if count_w > 0 && count_w <= rect.width {
                        let cx = rect.x + rect.width - count_w;
                        buf.set_line(
                            cx,
                            y,
                            &Line::from(Span::styled(
                                count_str,
                                Style::default().fg(self.theme.text_secondary),
                            )),
                            count_w,
                        );
                    }

                    if sel_row == Some(idx) {
                        buf.set_style(
                            Rect {
                                x: rect.x,
                                y,
                                width: rect.width,
                                height: 1,
                            },
                            sel_style,
                        );
                    }

                    hit_regions.push(BoardHitRegion {
                        rect: Rect {
                            x: rect.x,
                            y,
                            width: rect.width,
                            height: 1,
                        },
                        pos: BoardPos { col: 0, row: idx },
                    });
                }
            }
        }
    }

    /// One heading line for a server group in the sidebar: a health-coloured
    /// dot, the server name, and a status note. Degraded greys the name and
    /// shows the reason so a down server reads as inert, not active.
    fn render_server_heading(
        &self,
        server: &claude_commander_core::session::BoardServer,
        rect: Rect,
        y: u16,
        buf: &mut Buffer,
    ) {
        use claude_commander_core::backend::ConnectionState;
        let (dot_color, name_style, note) = match &server.connection {
            ConnectionState::Connected => (
                self.theme.status_running,
                Style::default()
                    .fg(self.theme.text_primary)
                    .add_modifier(Modifier::BOLD),
                None,
            ),
            ConnectionState::Connecting => (
                self.theme.text_secondary,
                Style::default()
                    .fg(self.theme.text_secondary)
                    .add_modifier(Modifier::BOLD),
                Some(("connecting…".to_string(), self.theme.text_secondary)),
            ),
            ConnectionState::Degraded { reason } => (
                self.theme.modal_warning,
                Style::default()
                    .fg(self.theme.text_secondary)
                    .add_modifier(Modifier::BOLD | Modifier::DIM),
                Some((reason.clone(), self.theme.modal_warning)),
            ),
        };
        let mut spans: Vec<Span<'static>> = vec![
            Span::styled("● ", Style::default().fg(dot_color)),
            Span::styled(server.name.clone(), name_style),
        ];
        if let Some((text, color)) = note {
            spans.push(Span::styled(
                format!(" ({text})"),
                Style::default().fg(color),
            ));
        }
        // A version-mismatch warning is independent of connection health: a
        // reachable-but-older server still shows a non-blocking `⚠ vX`.
        if let Some(mismatch) = &server.version_warning {
            spans.push(Span::styled(
                format!(" ⚠ v{}", mismatch.server),
                Style::default().fg(self.theme.modal_warning),
            ));
        }
        // Clickable affordance: opens Settings → Programs for this server (the
        // whole heading row is the click target).
        spans.push(Span::styled(
            " ⚙",
            Style::default().fg(self.theme.text_secondary),
        ));
        buf.set_line(rect.x, y, &Line::from(spans), rect.width);
    }

    /// Render one section column (addressable column `col_idx + 1`).
    #[allow(clippy::too_many_arguments)]
    fn render_column(
        &self,
        col_idx: usize,
        column: &claude_commander_core::session::BoardColumn,
        rect: Rect,
        buf: &mut Buffer,
        state: &mut BoardState,
        selected: Option<BoardPos>,
        numbers: Option<&HashMap<SessionId, usize>>,
        show_program: bool,
        hit_regions: &mut Vec<BoardHitRegion>,
        button_regions: &mut Vec<BoardButtonRegion>,
    ) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }

        self.render_column_header(column, rect, buf);
        if rect.height <= 1 {
            return; // only the header fits
        }

        let content = Rect {
            x: rect.x,
            y: rect.y + 1,
            width: rect.width,
            height: rect.height - 1,
        };
        let viewport_h = content.height as usize;

        // Every card is exactly one session row now → three display lines.
        let card_row_counts = vec![1usize; column.cards.len()];
        let ranges = layout::card_line_ranges(&card_row_counts);

        let addr_col = col_idx + 1;
        let sel_row = selected.filter(|p| p.col == addr_col).map(|p| p.row);

        // Scroll so the selected card is visible, then clamp within total lines.
        // One card == one row, so the row index is the card index directly.
        let mut sc = state.scroll[addr_col];
        if let Some(r) = sel_row
            && r < column.cards.len()
        {
            sc = layout::ensure_visible(sc, &ranges, r, 0, viewport_h);
        }
        let total_lines = ranges.last().map(|r| r.end).unwrap_or(0);
        sc = sc.min(total_lines.saturating_sub(viewport_h));
        state.scroll[addr_col] = sc;

        let content_top = content.y as isize;
        let content_bottom = (content.y + content.height) as isize;

        // Buttons show/hide uniformly per column, sized to the NARROWEST card
        // (an indented stacked child). Deciding per card would let a base card
        // keep its buttons while its own child loses them at borderline
        // widths — a confusing asymmetry within one stack.
        let column_min_inner = rect.width.saturating_sub(
            2 + if column.cards.iter().any(|c| c.indent) {
                CHILD_INDENT
            } else {
                0
            },
        );
        let column_buttons_fit =
            column_min_inner as usize > card_buttons_width() + (2 * CARD_PAD) as usize;

        for (row, card) in column.cards.iter().enumerate() {
            let range = &ranges[row];
            let card_top = content_top + range.start as isize - sc as isize;
            let card_h = (range.end - range.start) as u16;
            let card_bottom = card_top + card_h as isize;

            // Visible slice of this card, clipped to the lane's content area.
            let vis_top = card_top.max(content_top);
            let vis_bottom = card_bottom.min(content_bottom);
            if vis_bottom <= vis_top {
                continue;
            }

            // Stacked children shift right and narrow by CHILD_INDENT so they
            // read as nested under their base.
            let indent = if card.indent { CHILD_INDENT } else { 0 };
            let box_x = content.x + indent.min(rect.width);
            let box_w = rect.width.saturating_sub(indent);
            let visible_rect = Rect {
                x: box_x,
                y: vis_top as u16,
                width: box_w,
                height: (vis_bottom - vis_top) as u16,
            };

            let SessionListItem::Worktree { id, .. } = &card.row else {
                unreachable!("board rows are always Worktree")
            };
            let number = numbers.and_then(|m| m.get(id)).copied().unwrap_or(0);
            let (border_color, _) = self.project_color(card.project_id);
            self.render_card_border(&card.row, visible_rect, buf, number, border_color);

            // The interior line only renders when strictly inside the visible
            // box's top/bottom borders.
            let y = card_top + 1;
            if y > vis_top && y < vis_bottom - 1 {
                let yy = y as u16;
                let inner_x = box_x + 1;
                let inner_w = box_w.saturating_sub(2);
                let pos = BoardPos { col: addr_col, row };
                let is_selected = sel_row == Some(row);
                self.render_card_interior(
                    &card.row,
                    yy,
                    inner_x,
                    inner_w,
                    buf,
                    show_program,
                    is_selected,
                    pos,
                    column_buttons_fit,
                    button_regions,
                );
                hit_regions.push(BoardHitRegion {
                    rect: Rect {
                        x: rect.x,
                        y: yy,
                        width: rect.width,
                        height: 1,
                    },
                    pos,
                });
            }
        }

        // An empty column has no session cards to click, yet the keyboard can
        // land on it (row 0 — the header position). Mirror that for the mouse:
        // map any click in the empty lane to the column's row-0 position.
        // Emitted only for empty columns, so a populated column's
        // below-last-card space keeps selecting nothing and never shadows a
        // real row region.
        if column.cards.is_empty() {
            hit_regions.push(BoardHitRegion {
                rect: content,
                pos: BoardPos {
                    col: addr_col,
                    row: 0,
                },
            });
        }
    }

    /// Render a column's header line: `Name (count)` or `(count/max)` in the
    /// warning colour at/over the WIP limit.
    fn render_column_header(
        &self,
        column: &claude_commander_core::session::BoardColumn,
        rect: Rect,
        buf: &mut Buffer,
    ) {
        let count: usize = column.cards.len();
        let (count_text, count_color) = match column.max_sessions {
            Some(limit) => {
                // Advisory WIP colouring: over the limit reads as an error,
                // exactly at it as a warning, under it as normal.
                let limit = limit as usize;
                let color = if count > limit {
                    self.theme.modal_error
                } else if count == limit {
                    self.theme.modal_warning
                } else {
                    self.theme.text_secondary
                };
                (format!(" ({}/{})", count, limit), color)
            }
            None => (format!(" ({})", count), self.theme.text_secondary),
        };
        let line = Line::from(vec![
            Span::styled(
                column.name.clone(),
                Style::default()
                    .fg(self.theme.text_accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(count_text, Style::default().fg(count_color)),
        ]);
        buf.set_line(rect.x, rect.y, &line, rect.width);
    }

    /// Render a card's border box into `visible_rect` (which may be a clipped
    /// slice of the full card). The border is project-coloured; the title is the
    /// session's number and title — project identity lives in the colour and the
    /// sidebar legend, so no project name appears here.
    fn render_card_border(
        &self,
        item: &SessionListItem,
        visible_rect: Rect,
        buf: &mut Buffer,
        number: usize,
        border_color: Color,
    ) {
        let SessionListItem::Worktree { title, .. } = item else {
            unreachable!("board rows are always Worktree")
        };
        let border_type = if self.rounded {
            BorderType::Rounded
        } else {
            BorderType::Plain
        };
        let title_line = Line::from(vec![
            Span::styled(
                format!(" {number} "),
                Style::default().fg(self.theme.text_secondary),
            ),
            Span::styled(
                format!("{title} "),
                Style::default()
                    .fg(border_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(Style::default().fg(border_color))
            .title(title_line);
        block.render(visible_rect, buf);
    }

    /// Render a card's single interior line at `y`, over the inner width
    /// `[inner_x, inner_x + inner_w)`: status glyph, row markers, PR pill /
    /// `[branch]`, optional `(program)` suffix on the left, and the three action
    /// buttons right-aligned. Left content is truncated before the buttons are
    /// sacrificed. Highlights the row when selected and injects the PR hyperlink
    /// over the badge. Records each visible button in `button_regions`.
    #[allow(clippy::too_many_arguments)]
    fn render_card_interior(
        &self,
        item: &SessionListItem,
        y: u16,
        inner_x: u16,
        inner_w: u16,
        buf: &mut Buffer,
        show_program: bool,
        selected: bool,
        pos: BoardPos,
        buttons_fit: bool,
        button_regions: &mut Vec<BoardButtonRegion>,
    ) {
        if inner_w == 0 {
            return;
        }
        let SessionListItem::Worktree {
            id,
            title,
            branch,
            status,
            program,
            pr_number,
            pr_url,
            pr_merged,
            pr_state,
            pr_draft,
            pr_labels,
            agent_state,
            unread,
            keep_alive,
            lfs_pulling,
            ..
        } = item
        else {
            unreachable!("board rows are always Worktree")
        };

        // Left content: glyph + markers + PR/branch + program (no title/number —
        // those live in the border title).
        let mut spans: Vec<Span<'static>> = Vec::new();
        if let Some((glyph, color)) = status_glyph::session_status_glyph(
            self.theme,
            self.tick,
            *status,
            *agent_state,
            *unread,
        ) {
            spans.push(Span::styled(glyph, Style::default().fg(color)));
            spans.push(Span::styled(
                format!(
                    " {}",
                    status_glyph::status_label(*status, *agent_state, *unread)
                ),
                Style::default().fg(color),
            ));
        }
        if self.session_has_comments(id) {
            spans.push(Span::styled(
                format!(" {COMMENT_MARKER}"),
                Style::default().fg(self.theme.diff_file_header),
            ));
        }
        if *keep_alive {
            spans.push(Span::styled(
                format!(" {KEEP_ALIVE_MARKER}"),
                Style::default().fg(self.theme.text_accent),
            ));
        }
        if let Some(shown_branch) = claude_commander_core::session::display_branch(title, branch) {
            spans.push(Span::styled(
                format!(" [{}]", shown_branch),
                Style::default().fg(self.theme.text_accent),
            ));
        }
        if let Some(pr_num) = pr_number {
            spans.extend(pr_colors::pr_pill_spans(
                self.theme,
                self.invert_pr_label_color,
                *pr_num,
                *pr_state,
                *pr_merged,
                *pr_draft,
                pr_labels,
                self.review_labels,
            ));
        }
        if show_program {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("({})", status_glyph::program_name(program)),
                Style::default().fg(self.theme.text_secondary),
            ));
        }
        if *lfs_pulling {
            spans.push(Span::styled(
                LFS_MARKER,
                Style::default()
                    .fg(self.theme.text_secondary)
                    .add_modifier(Modifier::DIM),
            ));
        }

        // One pad column inside each border so content isn't flush against the
        // box, then reserve the right side for the action buttons; truncate
        // left content to whatever remains (at least a one-column gap before
        // the buttons). `buttons_fit` is decided per COLUMN (sized to its
        // narrowest card) so a stack's base and children gain/lose their
        // buttons together.
        let content_x = inner_x + CARD_PAD;
        let buttons_w = card_buttons_width();
        let (left_w, buttons_x) = if buttons_fit {
            let left = inner_w.saturating_sub(buttons_w as u16 + 2 * CARD_PAD + 1);
            (
                left,
                inner_x + inner_w.saturating_sub(buttons_w as u16 + CARD_PAD),
            )
        } else {
            // Too narrow for buttons: give the whole line to content.
            (inner_w.saturating_sub(2 * CARD_PAD), inner_x + inner_w)
        };

        if left_w > 0 {
            buf.set_line(content_x, y, &Line::from(spans), left_w);
        }

        if buttons_fit {
            self.render_card_buttons(buttons_x, y, buf, pos, selected, button_regions);
        }

        if selected {
            buf.set_style(
                Rect {
                    x: inner_x,
                    y,
                    width: inner_w,
                    height: 1,
                },
                self.theme.selection().add_modifier(Modifier::BOLD),
            );
        }

        if let (Some(pr_num), Some(url)) = (pr_number, pr_url)
            && left_w > 0
        {
            pr_colors::inject_pr_hyperlink(buf, y, inner_x, inner_x + left_w, *pr_num, url);
        }
    }

    /// Render the three action buttons starting at `x` and record a hit region
    /// per button. Subtle by default (secondary text); brightened on the
    /// selected card.
    fn render_card_buttons(
        &self,
        x: u16,
        y: u16,
        buf: &mut Buffer,
        pos: BoardPos,
        selected: bool,
        button_regions: &mut Vec<BoardButtonRegion>,
    ) {
        let style = if selected {
            Style::default()
                .fg(self.theme.text_primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.theme.text_secondary)
        };
        let mut bx = x;
        for (i, button) in CardButton::ORDER.iter().enumerate() {
            if i > 0 {
                bx += 1; // single-column gap between buttons
            }
            let label = button.label();
            let w = label.chars().count() as u16;
            buf.set_line(bx, y, &Line::from(Span::styled(label, style)), w);
            button_regions.push(BoardButtonRegion {
                rect: Rect {
                    x: bx,
                    y,
                    width: w,
                    height: 1,
                },
                pos,
                button: *button,
            });
            bx += w;
        }
    }
}

/// Total interior columns the three action buttons occupy, including the
/// single-column gaps between them.
fn card_buttons_width() -> usize {
    let labels: usize = CardButton::ORDER
        .iter()
        .map(|b| b.label().chars().count())
        .sum();
    labels + (CardButton::ORDER.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use claude_commander_core::session::{
        BoardCard, BoardColumn, BoardProjectEntry, SessionStatus,
    };
    use std::path::PathBuf;

    // --- construction helpers --------------------------------------------

    fn wt(project_id: ProjectId, title: &str, stacked_child: bool) -> SessionListItem {
        SessionListItem::Worktree {
            id: SessionId::new(),
            project_id,
            title: title.to_string(),
            branch: "feat".to_string(),
            status: SessionStatus::Running,
            program: "claude".to_string(),
            pr_number: None,
            pr_url: None,
            pr_merged: false,
            pr_state: None,
            pr_draft: false,
            pr_labels: Vec::new(),
            worktree_path: PathBuf::from("/tmp/wt"),
            created_at: Utc::now(),
            agent_state: None,
            unread: false,
            keep_alive: false,
            lfs_pulling: false,
            stacked_child,
        }
    }

    fn wt_id(item: &SessionListItem) -> SessionId {
        let SessionListItem::Worktree { id, .. } = item else {
            unreachable!("board rows are always Worktree")
        };
        *id
    }

    /// A single-session card. `indent` is derived from the item's
    /// `stacked_child` flag, mirroring `build_board`.
    fn card(pid: ProjectId, item: SessionListItem) -> BoardCard {
        let SessionListItem::Worktree { stacked_child, .. } = &item else {
            unreachable!("board rows are always Worktree")
        };
        let indent = *stacked_child;
        BoardCard {
            project_id: pid,
            project_name: "P".to_string(),
            row: item,
            indent,
        }
    }

    fn column(name: &str, max: Option<u32>, cards: Vec<BoardCard>) -> BoardColumn {
        BoardColumn {
            name: name.to_string(),
            max_sessions: max,
            cards,
        }
    }

    fn entry(pid: ProjectId, name: &str, count: usize) -> BoardProjectEntry {
        BoardProjectEntry {
            project_id: pid,
            name: name.to_string(),
            session_count: count,
        }
    }

    fn counts(board: &Board) -> Vec<usize> {
        board.selectable_row_counts()
    }

    fn color_map(board: &Board, theme: &Theme) -> HashMap<ProjectId, (Color, Color)> {
        board
            .projects
            .iter()
            .enumerate()
            .map(|(i, e)| (e.project_id, theme.project_color(i)))
            .collect()
    }

    // --- render harness --------------------------------------------------

    fn render(
        board: &Board,
        w: u16,
        h: u16,
        selected: Option<BoardPos>,
    ) -> (Buffer, BoardRenderOutput) {
        render_with(board, w, h, selected, |x| x)
    }

    fn render_with<F>(
        board: &Board,
        w: u16,
        h: u16,
        selected: Option<BoardPos>,
        configure: F,
    ) -> (Buffer, BoardRenderOutput)
    where
        F: for<'a> FnOnce(BoardWidget<'a>) -> BoardWidget<'a>,
    {
        let theme = Theme::basic();
        let colors = color_map(board, &theme);
        let numbers = board.session_numbers();
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        let mut state = BoardState::new();
        state.sync(counts(board));
        let widget = configure(
            BoardWidget::new(board, &theme)
                .selected(selected)
                .project_colors(&colors)
                .session_numbers(&numbers),
        );
        let out = widget.render(area, &mut buf, &mut state);
        (buf, out)
    }

    // --- buffer inspection helpers ---------------------------------------

    fn text_in_rect(buf: &Buffer, r: Rect) -> String {
        let mut s = String::new();
        for y in r.y..r.y + r.height {
            for x in r.x..r.x + r.width {
                s.push_str(buf[(x, y)].symbol());
            }
        }
        s
    }

    fn all_text(buf: &Buffer) -> String {
        text_in_rect(buf, buf.area)
    }

    fn first_non_space_x(buf: &Buffer, y: u16, x0: u16, x1: u16) -> Option<u16> {
        (x0..x1).find(|&x| buf[(x, y)].symbol() != " ")
    }

    // --- tests -----------------------------------------------------------

    #[test]
    fn card_border_uses_project_colour_and_shows_number_and_title_not_project_name() {
        let theme = Theme::basic();
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "MyProj", 1)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                vec![card(pid, wt(pid, "feature", false))],
            )],
        };
        let (buf, out) = render(&board, 80, 20, None);

        let expected_border = theme.project_color(0).0;
        let col = out.rects.columns[0];
        // Card top-left corner sits just below the header, at the column x.
        let corner = &buf[(col.x, col.y + 1)];
        assert_eq!(corner.symbol(), "┌");
        assert_eq!(
            corner.fg, expected_border,
            "border painted in project colour"
        );

        // The border title carries the session number and title; the project
        // name is never rendered on the card (identity lives in the colour + the
        // sidebar).
        let col_text = text_in_rect(&buf, col);
        assert!(
            col_text.contains('1'),
            "session number in the border title: {col_text}"
        );
        assert!(
            col_text.contains("feature"),
            "session title in the border title: {col_text}"
        );
        assert!(
            !col_text.contains("MyProj"),
            "project name must NOT appear on the card: {col_text}"
        );
    }

    #[test]
    fn status_glyph_and_column_major_numbering() {
        let pid = ProjectId::new();
        // In Progress has two worktrees; Open has one. Column-major numbering
        // makes the Open session number 3.
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 3)],
            columns: vec![
                column(
                    claude_commander_core::session::IN_PROGRESS,
                    None,
                    vec![
                        card(pid, wt(pid, "a", false)),
                        card(pid, wt(pid, "b", false)),
                    ],
                ),
                column("Open", None, vec![card(pid, wt(pid, "c", false))]),
            ],
        };
        let (buf, out) = render(&board, 90, 20, None);

        assert!(all_text(&buf).contains('●'), "running glyph rendered");

        // The Open column's only session is numbered 3 (after In Progress's 1
        // and 2), proving numbering is column-major across columns. The header
        // reads "Open (1)", so "3" only appears on the session row.
        let open_col = out.rects.columns[1];
        assert!(
            text_in_rect(&buf, open_col).contains('3'),
            "column-major numbering: Open's session is #3\n{}",
            text_in_rect(&buf, open_col)
        );
    }

    #[test]
    fn wip_limit_header_shows_count_over_max_in_warning_colour() {
        let theme = Theme::basic();
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 2)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                Some(2),
                vec![
                    card(pid, wt(pid, "a", false)),
                    card(pid, wt(pid, "b", false)),
                ],
            )],
        };
        let (buf, out) = render(&board, 60, 20, None);

        let col = out.rects.columns[0];
        assert!(
            text_in_rect(&buf, col).contains("(2/2)"),
            "at-limit header shows count/max"
        );
        // The "/" in the count is painted in the warning colour at the limit.
        let hy = col.y;
        let slash_x = (col.x..col.x + col.width)
            .find(|&x| buf[(x, hy)].symbol() == "/")
            .expect("count separator present");
        assert_eq!(buf[(slash_x, hy)].fg, theme.modal_warning);
    }

    #[test]
    fn under_limit_header_is_not_warning_coloured() {
        let theme = Theme::basic();
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 1)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                Some(5),
                vec![card(pid, wt(pid, "a", false))],
            )],
        };
        let (buf, out) = render(&board, 60, 20, None);
        let col = out.rects.columns[0];
        assert!(text_in_rect(&buf, col).contains("(1/5)"));
        let hy = col.y;
        let slash_x = (col.x..col.x + col.width)
            .find(|&x| buf[(x, hy)].symbol() == "/")
            .expect("count separator present");
        assert_eq!(buf[(slash_x, hy)].fg, theme.text_secondary);
    }

    #[test]
    fn stacked_child_card_is_indented_relative_to_base_card() {
        let pid = ProjectId::new();
        let base = wt(pid, "base", false);
        let child = wt(pid, "child", true);
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 2)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                // Two separate cards now: the base, then its indented child.
                vec![card(pid, base), card(pid, child)],
            )],
        };
        let (buf, out) = render(&board, 80, 20, None);

        let col = out.rects.columns[0];
        // Layout: header at col.y, base card at col.y+1..=+3 (top border, interior,
        // bottom border), child card at col.y+4.. — its top-left corner shifted
        // right by CHILD_INDENT.
        let base_corner_x = first_non_space_x(&buf, col.y + 1, col.x, col.x + col.width)
            .expect("base card top border");
        let child_corner_x = first_non_space_x(&buf, col.y + 4, col.x, col.x + col.width)
            .expect("child card top border");
        assert_eq!(base_corner_x, col.x, "base card sits flush with the column");
        assert_eq!(
            child_corner_x - base_corner_x,
            CHILD_INDENT,
            "stacked child card indented by exactly CHILD_INDENT"
        );
    }

    #[test]
    fn selected_session_row_is_highlighted() {
        let theme = Theme::basic();
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 1)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                vec![card(pid, wt(pid, "a", false))],
            )],
        };
        let sel = Some(BoardPos { col: 1, row: 0 });
        let (buf, out) = render(&board, 80, 20, sel);

        let col = out.rects.columns[0];
        let y = col.y + 2; // interior row
        let x = col.x + 1; // inner area
        assert_eq!(
            buf[(x, y)].bg,
            theme.selection_bg,
            "selected row painted with the selection background"
        );
        assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn selected_sidebar_row_is_highlighted() {
        let theme = Theme::basic();
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "Proj", 1)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                vec![card(pid, wt(pid, "a", false))],
            )],
        };
        let sel = Some(BoardPos { col: 0, row: 0 });
        let (buf, out) = render(&board, 80, 20, sel);

        let sb = out.rects.sidebar;
        assert_eq!(
            buf[(sb.x, sb.y)].bg,
            theme.selection_bg,
            "selected sidebar row highlighted"
        );
    }

    #[test]
    fn sidebar_lists_projects_with_counts_and_warns_when_pull_blocked() {
        let a = ProjectId::new();
        let z = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(a, "Alpha", 1), entry(z, "Zeta", 2)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                vec![card(a, wt(a, "s", false))],
            )],
        };
        let mut blocked: HashMap<ProjectId, BlockReason> = HashMap::new();
        blocked.insert(z, BlockReason::Diverged);
        // Rendered inline (not via the generic harness): the widget borrows the
        // local `blocked`, which a `for<'a>` configure closure can't express.
        let theme = Theme::basic();
        let colors = color_map(&board, &theme);
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        let mut state = BoardState::new();
        state.sync(counts(&board));
        let out = BoardWidget::new(&board, &theme)
            .project_colors(&colors)
            .pull_blocked_projects(&blocked)
            .render(area, &mut buf, &mut state);

        let sidebar_text = text_in_rect(&buf, out.rects.sidebar);
        assert!(sidebar_text.contains("Alpha"), "sidebar lists Alpha");
        assert!(sidebar_text.contains("Zeta"), "sidebar lists Zeta");
        assert!(
            sidebar_text.contains('1') && sidebar_text.contains('2'),
            "counts shown"
        );
        assert!(sidebar_text.contains('⚠'), "pull-blocked project warned");
    }

    #[test]
    fn hit_regions_map_one_to_one_to_visible_rows() {
        let pid = ProjectId::new();
        let a = wt(pid, "a", false);
        let b = wt(pid, "b", false);
        let c = wt(pid, "c", false);
        let (ida, idb, idc) = (wt_id(&a), wt_id(&b), wt_id(&c));
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 3)],
            columns: vec![
                column(
                    claude_commander_core::session::IN_PROGRESS,
                    None,
                    vec![card(pid, a), card(pid, b)],
                ),
                column("Open", None, vec![card(pid, c)]),
            ],
        };
        let (_buf, out) = render(&board, 90, 20, None);

        // One region per session row (3) plus one per sidebar row (1).
        assert_eq!(out.hit_regions.len(), 4);

        // Every session's board position is represented exactly once, and each
        // region is a single row within the drawn area.
        let session_positions: std::collections::HashSet<BoardPos> = out
            .hit_regions
            .iter()
            .filter(|r| r.pos.col != 0)
            .map(|r| r.pos)
            .collect();
        let expected: std::collections::HashSet<BoardPos> = [ida, idb, idc]
            .into_iter()
            .map(|id| board.position_of(id).unwrap())
            .collect();
        assert_eq!(session_positions, expected);

        for r in &out.hit_regions {
            assert_eq!(r.rect.height, 1);
            assert!(r.rect.y < 20);
        }

        // The sidebar region addresses column 0.
        assert!(
            out.hit_regions
                .iter()
                .any(|r| r.pos == BoardPos { col: 0, row: 0 })
        );
    }

    #[test]
    fn scrolled_column_offsets_hit_regions_into_the_viewport() {
        let pid = ProjectId::new();
        // Six single-row cards in one column; a short area forces scrolling.
        let cards: Vec<BoardCard> = (0..6)
            .map(|i| card(pid, wt(pid, &format!("s{i}"), false)))
            .collect();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 6)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                cards,
            )],
        };
        // Select the last row so the viewport scrolls to it.
        let sel = Some(BoardPos { col: 1, row: 5 });
        let (_buf, out) = render(&board, 60, 8, sel);

        // The selected row has a visible hit region inside the area...
        let selected_region = out
            .hit_regions
            .iter()
            .find(|r| r.pos == BoardPos { col: 1, row: 5 })
            .expect("selected row is visible after scrolling");
        assert!(selected_region.rect.y < 8);

        // ...and the scrolled-away first card's row is not rendered.
        assert!(
            !out.hit_regions
                .iter()
                .any(|r| r.pos == BoardPos { col: 1, row: 0 }),
            "row scrolled above the viewport should have no hit region"
        );
    }

    #[test]
    fn selecting_a_far_down_card_scrolls_it_into_view_and_is_stable() {
        let pid = ProjectId::new();
        // Twenty single-row cards (3 lines each) in a lane far shorter than the
        // stack — selecting one near the bottom must scroll it into view.
        let cards: Vec<BoardCard> = (0..20)
            .map(|i| card(pid, wt(pid, &format!("s{i}"), false)))
            .collect();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 20)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                cards,
            )],
        };
        let sel = Some(BoardPos { col: 1, row: 15 });

        // Render twice through the same state to prove the scroll is a fixed
        // point (no flip-flop) and the selected card stays visible.
        let theme = Theme::basic();
        let colors = color_map(&board, &theme);
        let area = Rect::new(0, 0, 60, 10);
        let mut state = BoardState::new();
        state.sync(counts(&board));

        let mut buf1 = Buffer::empty(area);
        let out1 = BoardWidget::new(&board, &theme)
            .selected(sel)
            .project_colors(&colors)
            .render(area, &mut buf1, &mut state);
        let scroll_after_first = state.scroll[1];
        assert!(
            out1.hit_regions
                .iter()
                .any(|r| r.pos == BoardPos { col: 1, row: 15 }),
            "selected card must have a visible hit region after scrolling"
        );

        let mut buf2 = Buffer::empty(area);
        let _ = BoardWidget::new(&board, &theme)
            .selected(sel)
            .project_colors(&colors)
            .render(area, &mut buf2, &mut state);
        assert_eq!(
            state.scroll[1], scroll_after_first,
            "scroll is stable across consecutive renders (no flip-flop)"
        );
    }

    #[test]
    fn empty_column_emits_a_lane_hit_region() {
        let pid = ProjectId::new();
        // In Progress has a session; the "Open" column is empty.
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 1)],
            columns: vec![
                column(
                    claude_commander_core::session::IN_PROGRESS,
                    None,
                    vec![card(pid, wt(pid, "a", false))],
                ),
                column("Open", None, vec![]),
            ],
        };
        let (_buf, out) = render(&board, 90, 20, None);

        // The empty "Open" column (addressable col 2) is clickable: a region
        // maps its lane to row 0 (the header position the keyboard also lands
        // on), and a point inside that column resolves to it.
        let open_col = out.rects.columns[1];
        let region = out
            .hit_regions
            .iter()
            .find(|r| r.pos == BoardPos { col: 2, row: 0 })
            .expect("empty column emits a lane hit region");
        let mid_x = open_col.x + open_col.width / 2;
        let mid_y = open_col.y + open_col.height / 2;
        assert!(
            mid_x >= region.rect.x
                && mid_x < region.rect.x + region.rect.width
                && mid_y >= region.rect.y
                && mid_y < region.rect.y + region.rect.height,
            "a click in the empty column's lane falls inside its hit region"
        );
    }

    #[test]
    fn populated_column_has_no_lane_region_below_its_last_card() {
        let pid = ProjectId::new();
        // A single short card in a tall lane leaves empty space below it.
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 1)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                vec![card(pid, wt(pid, "a", false))],
            )],
        };
        let (_buf, out) = render(&board, 60, 20, None);

        // Only the one session row is clickable in this column; the empty space
        // below the card selects nothing (a populated column gets no lane
        // fallback region — only truly empty columns do).
        let col_regions: Vec<_> = out.hit_regions.iter().filter(|r| r.pos.col == 1).collect();
        assert_eq!(
            col_regions.len(),
            1,
            "exactly one clickable row, no lane region"
        );
        assert_eq!(col_regions[0].pos, BoardPos { col: 1, row: 0 });
    }

    #[test]
    fn separators_drawn_in_border_colour_at_layout_positions() {
        let theme = Theme::basic();
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 1)],
            columns: vec![
                column(claude_commander_core::session::IN_PROGRESS, None, vec![]),
                column("Open", None, vec![]),
            ],
        };
        let (buf, out) = render(&board, 90, 12, None);

        assert!(!out.rects.separators.is_empty());
        for &sx in &out.rects.separators {
            let cell = &buf[(sx, out.rects.sidebar.y)];
            assert_eq!(cell.symbol(), "│", "separator glyph drawn at layout x");
            assert_eq!(cell.fg, theme.border_unfocused);
        }
    }

    #[test]
    fn narrow_area_does_not_panic() {
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 1)],
            columns: vec![
                column(
                    claude_commander_core::session::IN_PROGRESS,
                    Some(1),
                    vec![card(pid, wt(pid, "a", false))],
                ),
                column("In Review", None, vec![]),
                column("Merged", None, vec![]),
            ],
        };
        // 20x10 with a 24-wide sidebar leaves near-zero column width.
        let _ = render(&board, 20, 10, Some(BoardPos { col: 1, row: 0 }));
        // A zero-size area must also be safe.
        let _ = render(&board, 0, 0, None);
    }

    #[test]
    fn sidebar_scroll_keeps_server_heading_visible_with_selected_first_project() {
        use claude_commander_core::backend::{BackendId, ConnectionState};
        // Two servers, several projects each, in a sidebar too short for all
        // display lines. Selecting the FIRST project under the second server's
        // heading must scroll the heading into view too.
        let mk_entry = |name: &str| BoardProjectEntry {
            project_id: ProjectId::new(),
            name: name.to_string(),
            session_count: 0,
        };
        let projects: Vec<BoardProjectEntry> = ["a1", "a2", "a3", "a4", "b1", "b2"]
            .iter()
            .map(|n| mk_entry(n))
            .collect();
        let board = Board {
            servers: vec![
                claude_commander_core::session::BoardServer {
                    backend: BackendId(0),
                    name: "local".to_string(),
                    connection: ConnectionState::Connected,
                    version_warning: None,
                    projects: 0..4,
                },
                claude_commander_core::session::BoardServer {
                    backend: BackendId(1),
                    name: "buildbox".to_string(),
                    connection: ConnectionState::Connected,
                    version_warning: None,
                    projects: 4..6,
                },
            ],
            projects,
            columns: vec![BoardColumn {
                name: claude_commander_core::session::IN_PROGRESS.to_string(),
                max_sessions: None,
                cards: Vec::new(),
            }],
        };
        let theme = Theme::default();
        let mut state = BoardState::default();
        state.sync(vec![board.projects.len(), 0]);
        // Start scrolled well past the first server's heading, then select the
        // FIRST project under it (row 0; display line 1, heading at line 0).
        // Scrolling back up must bring the heading with it, not stop at the
        // project line.
        state.scroll[0] = 4;
        state.select(Some(BoardPos { col: 0, row: 0 }));
        let area = Rect::new(0, 0, 40, 4);
        let mut buf = Buffer::empty(area);
        let widget = BoardWidget::new(&board, &theme).selected(Some(BoardPos { col: 0, row: 0 }));
        let _ = widget.render(area, &mut buf, &mut state);
        let text: Vec<String> = (0..4)
            .map(|y| {
                (0..40)
                    .map(|x| buf[(x as u16, y as u16)].symbol().to_string())
                    .collect::<String>()
            })
            .collect();
        let joined = text.join("\n");
        assert!(
            joined.contains("local"),
            "server heading must scroll back into view above its selected first project: {joined}"
        );
        assert!(joined.contains("a1"), "selected project visible: {joined}");
    }

    // --- per-session cards: body glyph + action buttons ------------------

    #[test]
    fn status_glyph_renders_in_the_card_body_line() {
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 1)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                vec![card(pid, wt(pid, "feature", false))],
            )],
        };
        let (buf, out) = render(&board, 80, 20, None);

        // Interior line is the third row of the card (header, top border, body).
        let col = out.rects.columns[0];
        let body_y = col.y + 2;
        let body = text_in_rect(
            &buf,
            Rect {
                x: col.x,
                y: body_y,
                width: col.width,
                height: 1,
            },
        );
        assert!(
            body.contains('●'),
            "running glyph rendered in the card body line: {body}"
        );
    }

    #[test]
    fn card_renders_three_action_buttons_with_hit_regions() {
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 1)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                vec![card(pid, wt(pid, "feature", false))],
            )],
        };
        let (buf, out) = render(&board, 80, 20, None);

        // All three button glyphs render on the card body line.
        let all = all_text(&buf);
        assert!(all.contains("[>_]"), "shell button rendered: {all}");
        assert!(all.contains("[±]"), "review button rendered: {all}");
        assert!(all.contains("[i]"), "info button rendered: {all}");

        // One region per button, all for the card at (col 1, row 0), each a
        // single row, in Shell/Review/Info order left-to-right.
        let regions: Vec<_> = out
            .button_regions
            .iter()
            .filter(|r| r.pos == BoardPos { col: 1, row: 0 })
            .collect();
        assert_eq!(regions.len(), 3, "three button regions on the card");
        assert_eq!(regions[0].button, CardButton::Shell);
        assert_eq!(regions[1].button, CardButton::Review);
        assert_eq!(regions[2].button, CardButton::Info);
        for r in &regions {
            assert_eq!(r.rect.height, 1);
        }
        // Left-to-right, non-overlapping.
        assert!(regions[0].rect.x < regions[1].rect.x);
        assert!(regions[1].rect.x < regions[2].rect.x);
        assert!(regions[0].rect.x + regions[0].rect.width <= regions[1].rect.x);
    }

    #[test]
    fn button_region_maps_to_the_right_card_and_button() {
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 2)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                vec![
                    card(pid, wt(pid, "a", false)),
                    card(pid, wt(pid, "b", false)),
                ],
            )],
        };
        let (_buf, out) = render(&board, 80, 20, None);

        // The second card's review button belongs to (col 1, row 1) and its rect
        // lands on that card's body line.
        let review = out
            .button_regions
            .iter()
            .find(|r| r.pos == BoardPos { col: 1, row: 1 } && r.button == CardButton::Review)
            .expect("second card has a review button");
        let row_region = out
            .hit_regions
            .iter()
            .find(|r| r.pos == BoardPos { col: 1, row: 1 })
            .expect("second card has a row region");
        assert_eq!(
            review.rect.y, row_region.rect.y,
            "button sits on its card's body line"
        );
        // The button rect is contained within the row region horizontally — the
        // app resolves this overlap by checking buttons first.
        assert!(review.rect.x >= row_region.rect.x);
        assert!(review.rect.x + review.rect.width <= row_region.rect.x + row_region.rect.width);
    }

    #[test]
    fn indented_child_card_is_left_indented_with_column_aligned_buttons() {
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "P", 2)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                vec![
                    card(pid, wt(pid, "base", false)),
                    card(pid, wt(pid, "child", true)),
                ],
            )],
        };
        let (buf, out) = render(&board, 80, 20, None);

        let col = out.rects.columns[0];
        // Base card top border at col.y+1; child card top border at col.y+4.
        let base_left =
            first_non_space_x(&buf, col.y + 1, col.x, col.x + col.width).expect("base card border");
        let child_left = first_non_space_x(&buf, col.y + 4, col.x, col.x + col.width)
            .expect("child card border");
        assert_eq!(
            child_left - base_left,
            CHILD_INDENT,
            "child card left border indented by CHILD_INDENT"
        );

        // The child card narrows on the left only, so its right edge — and thus
        // its right-aligned action buttons — stay column-aligned with the base
        // card's. The buttons are never sacrificed to the indent.
        let info_right = |row: usize| {
            out.button_regions
                .iter()
                .find(|r| r.pos == BoardPos { col: 1, row } && r.button == CardButton::Info)
                .map(|r| r.rect.x + r.rect.width)
                .expect("info button present")
        };
        assert_eq!(
            info_right(0),
            info_right(1),
            "base and child action buttons share the same right edge"
        );
    }

    #[test]
    fn stack_base_and_child_agree_on_button_visibility_at_borderline_width() {
        // At a column width where a base card's interior fits the buttons but
        // an indented child's would not, NEITHER shows buttons — visibility is
        // decided per column on its narrowest card, so a stack never renders a
        // confusing base-has-buttons/child-does-not asymmetry.
        let pid = ProjectId::new();
        let base = BoardCard {
            project_id: pid,
            project_name: "p".to_string(),
            row: wt(pid, "base", false),
            indent: false,
        };
        let child = BoardCard {
            project_id: pid,
            project_name: "p".to_string(),
            row: wt(pid, "child", true),
            indent: true,
        };
        let board = Board {
            servers: vec![],
            projects: vec![BoardProjectEntry {
                project_id: pid,
                name: "p".to_string(),
                session_count: 2,
            }],
            columns: vec![BoardColumn {
                name: claude_commander_core::session::IN_PROGRESS.to_string(),
                max_sessions: None,
                cards: vec![base, child],
            }],
        };
        let theme = Theme::default();
        // Width chosen so base inner (col-2) > buttons_w but child inner
        // (col-2-CHILD_INDENT) <= buttons_w.
        let buttons_w = card_buttons_width() as u16;
        let col_w = buttons_w + 3; // base inner = buttons_w+1 (fits), child inner = buttons_w-1 (doesn't)
        let sidebar_and_sep = crate::widgets::board::layout::SIDEBAR_WIDTH + 1;
        let area = Rect::new(0, 0, sidebar_and_sep + col_w, 10);
        let mut state = BoardState::default();
        state.sync(vec![1, 2]);
        let mut buf = Buffer::empty(area);
        let out = BoardWidget::new(&board, &theme).render(area, &mut buf, &mut state);
        assert!(
            out.button_regions.is_empty(),
            "no card in the column should render buttons when the narrowest card cannot fit them: {:?}",
            out.button_regions
        );
    }

    #[test]
    fn multi_server_sidebar_headings_render_gear_and_emit_heading_regions() {
        use claude_commander_core::backend::{BackendId, ConnectionState};
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![
                claude_commander_core::session::BoardServer {
                    backend: BackendId(0),
                    name: "local".to_string(),
                    connection: ConnectionState::Connected,
                    version_warning: None,
                    projects: 0..1,
                },
                claude_commander_core::session::BoardServer {
                    backend: BackendId(1),
                    name: "buildbox".to_string(),
                    connection: ConnectionState::Connected,
                    version_warning: None,
                    projects: 1..1,
                },
            ],
            projects: vec![BoardProjectEntry {
                project_id: pid,
                name: "p".to_string(),
                session_count: 0,
            }],
            columns: vec![BoardColumn {
                name: claude_commander_core::session::IN_PROGRESS.to_string(),
                max_sessions: None,
                cards: Vec::new(),
            }],
        };
        let theme = Theme::default();
        let mut state = BoardState::default();
        state.sync(vec![1, 0]);
        let area = Rect::new(0, 0, 80, 12);
        let mut buf = Buffer::empty(area);
        let out = BoardWidget::new(&board, &theme).render(area, &mut buf, &mut state);

        let backends: Vec<_> = out.heading_regions.iter().map(|(_, b)| *b).collect();
        assert_eq!(
            backends,
            vec![BackendId(0), BackendId(1)],
            "one heading region per server, in order"
        );
        // The ⚙ affordance renders on the heading line.
        let (rect, _) = out.heading_regions[0];
        let row: String = (rect.x..rect.x + rect.width)
            .map(|x| buf[(x, rect.y)].symbol().to_string())
            .collect();
        assert!(row.contains('⚙'), "heading row should show the ⚙: {row}");
    }

    #[test]
    fn card_pads_title_and_body_and_shows_status_word() {
        let pid = ProjectId::new();
        let card = BoardCard {
            project_id: pid,
            project_name: "p".to_string(),
            row: wt(pid, "review", false),
            indent: false,
        };
        let board = Board {
            servers: vec![],
            projects: vec![BoardProjectEntry {
                project_id: pid,
                name: "p".to_string(),
                session_count: 1,
            }],
            columns: vec![BoardColumn {
                name: claude_commander_core::session::IN_PROGRESS.to_string(),
                max_sessions: None,
                cards: vec![card],
            }],
        };
        let theme = Theme::default();
        let mut state = BoardState::default();
        state.sync(vec![1, 1]);
        let area = Rect::new(0, 0, 80, 8);
        let mut buf = Buffer::empty(area);
        let numbers = board.session_numbers();
        let out = BoardWidget::new(&board, &theme)
            .session_numbers(&numbers)
            .render(area, &mut buf, &mut state);

        let row_text =
            |y: u16| -> String { (0..80).map(|x| buf[(x, y)].symbol().to_string()).collect() };
        // The card's interior hit region (col 1 — col 0 regions are sidebar
        // rows) pins its body line; the border with the title sits above it.
        let body_y = out
            .hit_regions
            .iter()
            .find(|r| r.pos.col == 1)
            .expect("card hit region")
            .rect
            .y;
        // Border title has a space either side: "╭ 1 review ─", never "╭1 review─".
        let border = row_text(body_y - 1);
        assert!(
            border.contains(" 1 review ─"),
            "border title must be padded on both sides: {border}"
        );
        // Body line: one pad column inside the left border, then glyph + word.
        let body = row_text(body_y);
        let col_x = out.rects.columns[0].x as usize;
        let inside: String = body.chars().skip(col_x + 1).collect();
        assert!(
            inside.starts_with(' '),
            "body content must not be flush against the left border: {body}"
        );
        assert!(
            body.contains("● idle"),
            "glyph should be followed by its status word: {body}"
        );
        // Buttons end one column short of the right border.
        let btn = out
            .button_regions
            .iter()
            .map(|r| r.rect.x + r.rect.width)
            .max()
            .expect("buttons rendered");
        let right_border = out.rects.columns[0].x + out.rects.columns[0].width - 1;
        assert_eq!(
            btn + 1,
            right_border,
            "buttons must leave one pad column before the right border"
        );
    }

    // --- snapshots -------------------------------------------------------
    //
    // The assertions above check individual invariants (a glyph is present, a
    // rect lines up). These snapshot whole frames, which is what catches the
    // failures targeted assertions miss: a column width that shifts, borders
    // that stop joining, a card that silently loses a line. They live here
    // rather than in `crate::render_tests` so they reuse the fixture builders
    // and `render` harness above instead of duplicating them.
    //
    // `BoardWidget::render` writes into a `Buffer` and returns hit-test output,
    // so it is not a `StatefulWidget` and cannot go through
    // `Frame::render_stateful_widget` the way the widgets in
    // `crate::render_tests` do — hence rendering into a buffer and formatting it
    // the way ratatui's `TestBackend` would.

    /// Format a `Buffer` as one quoted string per row, matching `TestBackend`'s
    /// `Display` so board snapshots read like the other render snapshots.
    ///
    /// Captures **symbols only**, not styles — same limitation as `TestBackend`.
    /// So these snapshots pin geometry, truncation and glyph choice, while colour
    /// and highlighting stay the job of the targeted assertions above
    /// (`selected_session_row_is_highlighted`, `selected_sidebar_row_is_highlighted`).
    fn buffer_snapshot(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in buf.area.y..buf.area.y + buf.area.height {
            out.push('"');
            for x in buf.area.x..buf.area.x + buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push_str("\"\n");
        }
        out
    }

    #[test]
    fn snapshot_board_single_column() {
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "my-app", 1)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                vec![card(pid, wt(pid, "add-auth", false))],
            )],
        };
        let (buf, _) = render(&board, 80, 14, None);
        insta::assert_snapshot!(buffer_snapshot(&buf));
    }

    /// Three columns at a width where each is narrow enough to truncate a card's
    /// branch label — the case where column arithmetic is easiest to get wrong.
    /// A selection is set so the layout is snapshotted in the state the user
    /// actually sees, but the highlight itself is a style and so invisible here;
    /// `selected_session_row_is_highlighted` covers that.
    #[test]
    fn snapshot_board_multi_column_layout() {
        let pid = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "my-app", 3)],
            columns: vec![
                column(
                    claude_commander_core::session::IN_PROGRESS,
                    None,
                    vec![
                        card(pid, wt(pid, "add-auth", false)),
                        card(pid, wt(pid, "fix-login", false)),
                    ],
                ),
                column(
                    "In Review",
                    None,
                    vec![card(pid, wt(pid, "refactor", false))],
                ),
                column("Merged", None, vec![]),
            ],
        };
        // Column 1 is the sidebar, so BoardPos { column: 1, .. } is the first
        // real column — select its second card.
        let (buf, _) = render(&board, 110, 16, Some(BoardPos { col: 1, row: 1 }));
        insta::assert_snapshot!(buffer_snapshot(&buf));
    }

    #[test]
    fn snapshot_board_stacked_card() {
        let pid = ProjectId::new();
        // A `stacked_child` row gets its own border, indented two columns and
        // correspondingly narrower, so the nesting is visible while the stack
        // still reads as one unit. Snapshotted because that indent-plus-inset
        // arithmetic is what silently drifts.
        let board = Board {
            servers: vec![],
            projects: vec![entry(pid, "my-app", 2)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                vec![
                    card(pid, wt(pid, "parent-feature", false)),
                    card(pid, wt(pid, "stacked-child", true)),
                ],
            )],
        };
        let (buf, _) = render(&board, 80, 14, None);
        insta::assert_snapshot!(buffer_snapshot(&buf));
    }

    #[test]
    fn snapshot_board_sidebar_with_two_projects() {
        let a = ProjectId::new();
        let z = ProjectId::new();
        let board = Board {
            servers: vec![],
            projects: vec![entry(a, "alpha", 1), entry(z, "zeta", 2)],
            columns: vec![column(
                claude_commander_core::session::IN_PROGRESS,
                None,
                vec![card(a, wt(a, "one", false))],
            )],
        };
        let (buf, _) = render(&board, 90, 12, None);
        insta::assert_snapshot!(buffer_snapshot(&buf));
    }
}
