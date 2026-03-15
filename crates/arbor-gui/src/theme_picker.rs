use super::*;

impl ArborWindow {
    pub(crate) fn open_theme_picker_modal(&mut self, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        self.show_theme_picker = true;
        self.theme_picker_selected_index = theme_picker_index_for_kind(self.theme_kind);
        self.sync_theme_picker_scroll();
        cx.defer(move |cx| {
            let _ = this.update(cx, |this, cx| {
                if this.show_theme_picker {
                    this.sync_theme_picker_scroll();
                    cx.notify();
                }
            });
        });
        cx.notify();
    }

    pub(crate) fn move_theme_picker_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = ThemeKind::ALL.len();
        if len == 0 {
            return;
        }
        let current = self.theme_picker_selected_index.min(len - 1) as isize;
        self.theme_picker_selected_index = (current + delta).rem_euclid(len as isize) as usize;
        self.sync_theme_picker_scroll();
        cx.notify();
    }

    pub(crate) fn apply_selected_theme_picker_theme(&mut self, cx: &mut Context<Self>) {
        let Some(&kind) = ThemeKind::ALL.get(self.theme_picker_selected_index) else {
            return;
        };
        self.switch_theme(kind, cx);
    }

    pub(crate) fn render_theme_picker_modal(&mut self, cx: &mut Context<Self>) -> Div {
        if !self.show_theme_picker {
            return div();
        }

        let theme = self.theme();
        let theme_count = ThemeKind::ALL.len();
        let columns = theme_picker_columns(theme_count);
        let visible_rows = theme_picker_visible_rows(theme_count, columns);
        let modal_width = theme_picker_modal_width_px(columns);
        let modal_height = theme_picker_modal_height_px(visible_rows);
        let grid_width = theme_picker_grid_width_px(columns);
        let current_theme = self.theme_kind;
        let selected_index = self
            .theme_picker_selected_index
            .min(theme_count.saturating_sub(1));

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.show_theme_picker = false;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.show_theme_picker = false;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(modal_width))
                    .max_w(px(THEME_PICKER_MAX_MODAL_WIDTH_PX))
                    .h(px(modal_height))
                    .max_h(px(THEME_PICKER_MAX_MODAL_HEIGHT_PX))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_primary))
                            .child("Choose Theme"),
                    )
                    .child(
                        div()
                            .id("theme-picker-grid-scroll")
                            .w_full()
                            .flex_1()
                            .min_h_0()
                            .px(px(THEME_PICKER_GRID_SIDE_INSET_PX))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_2()
                            .overflow_y_scroll()
                            .scrollbar_width(px(THEME_PICKER_SCROLLBAR_WIDTH_PX))
                            .track_scroll(&self.theme_picker_scroll_handle)
                            .children(ThemeKind::ALL.chunks(columns).enumerate().map(
                                |(row_index, row)| {
                                    div()
                                        .id(("theme-picker-row", row_index))
                                        .w(px(grid_width))
                                        .flex()
                                        .gap_2()
                                        .children(row.iter().enumerate().map(
                                            |(column_index, &kind)| {
                                                let idx = row_index * columns + column_index;
                                                let palette = kind.palette();
                                                let is_active = kind == current_theme;
                                                let is_selected = idx == selected_index;
                                                let border_color = if is_selected || is_active {
                                                    theme.accent
                                                } else {
                                                    theme.border
                                                };
                                                div()
                                                    .id(("theme-card", idx))
                                                    .w(px(THEME_PICKER_CARD_WIDTH_PX))
                                                    .h(px(THEME_PICKER_CARD_HEIGHT_PX))
                                                    .rounded_md()
                                                    .border_1()
                                                    .border_color(rgb(border_color))
                                                    .when(is_active || is_selected, |d| {
                                                        d.border_2()
                                                    })
                                                    .bg(rgb(if is_selected {
                                                        theme.panel_active_bg
                                                    } else {
                                                        theme.panel_bg
                                                    }))
                                                    .overflow_hidden()
                                                    .cursor_pointer()
                                                    .hover(|s| s.opacity(0.85))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.theme_picker_selected_index = idx;
                                                        this.sync_theme_picker_scroll();
                                                        this.switch_theme(kind, cx);
                                                    }))
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .flex_row()
                                                            .h(px(
                                                                THEME_PICKER_CARD_PREVIEW_HEIGHT_PX,
                                                            ))
                                                            .child(
                                                                div()
                                                                    .flex_1()
                                                                    .bg(rgb(palette.app_bg)),
                                                            )
                                                            .child(
                                                                div()
                                                                    .flex_1()
                                                                    .bg(rgb(palette.sidebar_bg)),
                                                            )
                                                            .child(
                                                                div()
                                                                    .flex_1()
                                                                    .bg(rgb(palette.accent)),
                                                            )
                                                            .child(
                                                                div()
                                                                    .flex_1()
                                                                    .bg(rgb(palette.text_primary)),
                                                            )
                                                            .child(
                                                                div()
                                                                    .flex_1()
                                                                    .bg(rgb(palette.border)),
                                                            ),
                                                    )
                                                    .child(
                                                        div()
                                                            .px_2()
                                                            .py(px(6.))
                                                            .h(px(
                                                                THEME_PICKER_CARD_LABEL_HEIGHT_PX,
                                                            ))
                                                            .text_xs()
                                                            .text_color(rgb(theme.text_primary))
                                                            .when(is_active || is_selected, |d| {
                                                                d.font_weight(FontWeight::SEMIBOLD)
                                                            })
                                                            .child(kind.label()),
                                                    )
                                            },
                                        ))
                                },
                            )),
                    ),
            )
    }

    pub(crate) fn sync_theme_picker_scroll(&self) {
        let theme_count = ThemeKind::ALL.len();
        let columns = theme_picker_columns(theme_count);
        let total_rows = theme_picker_rows(theme_count, columns);
        let visible_rows = theme_picker_visible_rows(theme_count, columns);
        let selected_row = self
            .theme_picker_selected_index
            .min(theme_count.saturating_sub(1))
            / columns;
        let max_top_row = total_rows.saturating_sub(visible_rows);
        let top_row = selected_row
            .saturating_sub(visible_rows / 2)
            .min(max_top_row);
        self.theme_picker_scroll_handle
            .scroll_to_top_of_item(top_row);
    }
}

pub(crate) const THEME_PICKER_CARD_WIDTH_PX: f32 = 148.;
pub(crate) const THEME_PICKER_CARD_PREVIEW_HEIGHT_PX: f32 = 36.;
pub(crate) const THEME_PICKER_CARD_LABEL_HEIGHT_PX: f32 = 44.;
pub(crate) const THEME_PICKER_CARD_HEIGHT_PX: f32 =
    THEME_PICKER_CARD_PREVIEW_HEIGHT_PX + THEME_PICKER_CARD_LABEL_HEIGHT_PX;
pub(crate) const THEME_PICKER_CARD_GAP_PX: f32 = 8.;
pub(crate) const THEME_PICKER_GRID_SIDE_INSET_PX: f32 = 8.;
pub(crate) const THEME_PICKER_MODAL_PADDING_PX: f32 = 16.;
pub(crate) const THEME_PICKER_MODAL_HEADER_HEIGHT_PX: f32 = 20.;
pub(crate) const THEME_PICKER_MODAL_SECTION_GAP_PX: f32 = 12.;
pub(crate) const THEME_PICKER_SCROLLBAR_WIDTH_PX: f32 = 12.;
pub(crate) const THEME_PICKER_MAX_MODAL_WIDTH_PX: f32 = 960.;
pub(crate) const THEME_PICKER_MAX_VISIBLE_ROWS: usize = 5;
pub(crate) const THEME_PICKER_MAX_MODAL_HEIGHT_PX: f32 = 760.;

pub(crate) fn theme_picker_columns(theme_count: usize) -> usize {
    match theme_count {
        0..=4 => theme_count.max(1),
        5..=12 => 4,
        _ => 5,
    }
}

pub(crate) fn theme_picker_rows(theme_count: usize, columns: usize) -> usize {
    theme_count.max(1).div_ceil(columns.max(1))
}

pub(crate) fn theme_picker_visible_rows(theme_count: usize, columns: usize) -> usize {
    theme_picker_rows(theme_count, columns).clamp(1, THEME_PICKER_MAX_VISIBLE_ROWS)
}

pub(crate) fn theme_picker_grid_width_px(columns: usize) -> f32 {
    let columns = columns.max(1) as f32;
    let card_span = columns * THEME_PICKER_CARD_WIDTH_PX;
    let gutter_span = (columns - 1.).max(0.) * THEME_PICKER_CARD_GAP_PX;
    card_span + gutter_span
}

pub(crate) fn theme_picker_modal_width_px(columns: usize) -> f32 {
    let chrome = (THEME_PICKER_MODAL_PADDING_PX * 2.) + (THEME_PICKER_GRID_SIDE_INSET_PX * 2.) + 2.;
    let width = theme_picker_grid_width_px(columns) + chrome;
    width.min(THEME_PICKER_MAX_MODAL_WIDTH_PX)
}

pub(crate) fn theme_picker_modal_height_px(visible_rows: usize) -> f32 {
    let rows = visible_rows.max(1) as f32;
    let row_span = rows * THEME_PICKER_CARD_HEIGHT_PX;
    let gutter_span = (rows - 1.).max(0.) * THEME_PICKER_CARD_GAP_PX;
    let chrome = (THEME_PICKER_MODAL_PADDING_PX * 2.)
        + THEME_PICKER_MODAL_HEADER_HEIGHT_PX
        + THEME_PICKER_MODAL_SECTION_GAP_PX
        + 2.;
    (row_span + gutter_span + chrome).min(THEME_PICKER_MAX_MODAL_HEIGHT_PX)
}

pub(crate) fn theme_picker_index_for_kind(theme_kind: ThemeKind) -> usize {
    ThemeKind::ALL
        .iter()
        .position(|candidate| *candidate == theme_kind)
        .unwrap_or(0)
}
