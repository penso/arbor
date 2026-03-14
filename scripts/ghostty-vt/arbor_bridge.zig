const std = @import("std");
const lib = @import("ghostty-vt");

const Allocator = std.mem.Allocator;
const page_allocator = std.heap.page_allocator;

const CountingReadonlyStream = lib.Stream(CountingReadonlyHandler);

const CountingReadonlyHandler = struct {
    readonly: lib.ReadonlyHandler,
    bell_count: usize = 0,

    pub fn init(terminal: *lib.Terminal) CountingReadonlyHandler {
        return .{
            .readonly = lib.ReadonlyHandler.init(terminal),
        };
    }

    pub fn deinit(self: *CountingReadonlyHandler) void {
        self.readonly.deinit();
    }

    pub fn vt(
        self: *CountingReadonlyHandler,
        comptime action: lib.StreamAction.Tag,
        value: lib.StreamAction.Value(action),
    ) !void {
        if (action == .bell) self.bell_count += 1;
        try self.readonly.vt(action, value);
    }

    fn takeBellCount(self: *CountingReadonlyHandler) usize {
        const count = self.bell_count;
        self.bell_count = 0;
        return count;
    }
};

const State = struct {
    alloc: Allocator,
    terminal: lib.Terminal,
    stream: CountingReadonlyStream,
};

pub const ArborGhosttyBuffer = extern struct {
    ptr: [*]u8,
    len: usize,
};

pub const ArborGhosttyStyledLine = extern struct {
    cell_start: usize,
    cell_len: usize,
};

pub const ArborGhosttyStyledCell = extern struct {
    column: usize,
    text_offset: usize,
    text_len: usize,
    fg: u32,
    bg: u32,
};

pub const ArborGhosttyStyledSnapshot = extern struct {
    lines_ptr: [*]ArborGhosttyStyledLine,
    lines_len: usize,
    cells_ptr: [*]ArborGhosttyStyledCell,
    cells_len: usize,
    text_ptr: [*]u8,
    text_len: usize,
    cursor_visible: bool,
    cursor_line: usize,
    cursor_column: usize,
    app_cursor: bool,
    alt_screen: bool,
};

const arbor_default_fg: lib.color.RGB = .{ .r = 0xab, .g = 0xb2, .b = 0xbf };
const arbor_default_bg: lib.color.RGB = .{ .r = 0x28, .g = 0x2c, .b = 0x34 };

fn ptrFromHandle(handle: ?*anyopaque) ?*State {
    const raw = handle orelse return null;
    return @ptrCast(@alignCast(raw));
}

fn writeSnapshot(
    state: *State,
    opts: lib.formatter.Options,
    extra: lib.formatter.TerminalFormatter.Extra,
    out: *ArborGhosttyBuffer,
) i32 {
    var builder: std.Io.Writer.Allocating = .init(state.alloc);
    defer builder.deinit();

    var formatter = lib.formatter.TerminalFormatter.init(&state.terminal, opts);
    formatter.extra = extra;
    formatter.format(&builder.writer) catch return 1;

    const snapshot = builder.writer.buffered();
    const owned = state.alloc.dupe(u8, snapshot) catch return 2;
    out.* = .{ .ptr = owned.ptr, .len = owned.len };
    return 0;
}

pub export fn arbor_ghostty_vt_new(
    rows: u16,
    cols: u16,
    scrollback: usize,
    out: *?*anyopaque,
) i32 {
    const alloc = page_allocator;
    const state = alloc.create(State) catch return 1;
    errdefer alloc.destroy(state);

    state.alloc = alloc;
    state.terminal = lib.Terminal.init(alloc, .{
        .cols = cols,
        .rows = rows,
        .max_scrollback = scrollback,
    }) catch return 2;
    errdefer state.terminal.deinit(alloc);

    state.stream = .initAlloc(alloc, CountingReadonlyHandler.init(&state.terminal));
    out.* = @ptrCast(state);
    return 0;
}

pub export fn arbor_ghostty_vt_free(handle: ?*anyopaque) void {
    const state = ptrFromHandle(handle) orelse return;
    state.stream.deinit();
    state.terminal.deinit(state.alloc);
    state.alloc.destroy(state);
}

pub export fn arbor_ghostty_vt_process(
    handle: ?*anyopaque,
    bytes: [*]const u8,
    len: usize,
    bell_count: *usize,
) i32 {
    const state = ptrFromHandle(handle) orelse return 1;
    bell_count.* = 0;
    if (len == 0) return 0;
    state.stream.nextSlice(bytes[0..len]) catch return 2;
    bell_count.* = state.stream.handler.takeBellCount();
    return 0;
}

pub export fn arbor_ghostty_vt_resize(handle: ?*anyopaque, rows: u16, cols: u16) i32 {
    const state = ptrFromHandle(handle) orelse return 1;
    state.terminal.resize(state.alloc, cols, rows) catch return 2;
    return 0;
}

pub export fn arbor_ghostty_vt_snapshot_plain(
    handle: ?*anyopaque,
    out: *ArborGhosttyBuffer,
) i32 {
    const state = ptrFromHandle(handle) orelse return 1;
    const extra = lib.formatter.TerminalFormatter.Extra.none;
    return writeSnapshot(state, .plain, extra, out);
}

pub export fn arbor_ghostty_vt_snapshot_vt(
    handle: ?*anyopaque,
    out: *ArborGhosttyBuffer,
) i32 {
    const state = ptrFromHandle(handle) orelse return 1;
    return writeSnapshot(state, .vt, .styles, out);
}

fn encodeCodepoint(buffer: *std.ArrayListUnmanaged(u8), alloc: Allocator, cp: u21) !void {
    var utf8: [4]u8 = undefined;
    const len = std.unicode.utf8Encode(cp, &utf8) catch {
        try buffer.append(alloc, ' ');
        return;
    };
    try buffer.appendSlice(alloc, utf8[0..len]);
}

fn rgbToU32(rgb: lib.color.RGB) u32 {
    return (@as(u32, rgb.r) << 16) | (@as(u32, rgb.g) << 8) | @as(u32, rgb.b);
}

fn resolveColors(
    state: *State,
    cell: *const lib.page.Cell,
    style: lib.Style,
) struct { fg: u32, bg: u32 } {
    const palette = &state.terminal.colors.palette.current;
    const default_fg = state.terminal.colors.foreground.get() orelse arbor_default_fg;
    const default_bg = state.terminal.colors.background.get() orelse arbor_default_bg;

    var fg = style.fg(.{
        .default = default_fg,
        .palette = palette,
    });
    var bg = style.bg(cell, palette) orelse default_bg;

    if (style.flags.inverse) {
        const swapped_fg = fg;
        fg = bg;
        bg = swapped_fg;
    }

    return .{
        .fg = rgbToU32(fg),
        .bg = rgbToU32(bg),
    };
}

fn currentScreenCursor(state: *State) struct { visible: bool, line: usize, column: usize } {
    if (!state.terminal.modes.get(.cursor_visible)) {
        return .{
            .visible = false,
            .line = 0,
            .column = 0,
        };
    }

    const pin = state.terminal.screens.active.cursor.page_pin.*;
    if (state.terminal.screens.active.pages.pointFromPin(.screen, pin)) |pt| {
        return .{
            .visible = true,
            .line = pt.screen.y,
            .column = pt.screen.x,
        };
    }

    return .{
        .visible = true,
        .line = state.terminal.screens.active.cursor.y,
        .column = state.terminal.screens.active.cursor.x,
    };
}

pub export fn arbor_ghostty_vt_snapshot_styled(
    handle: ?*anyopaque,
    out: *ArborGhosttyStyledSnapshot,
) i32 {
    const state = ptrFromHandle(handle) orelse return 1;
    const cursor = currentScreenCursor(state);

    var lines: std.ArrayListUnmanaged(ArborGhosttyStyledLine) = .empty;
    defer lines.deinit(state.alloc);

    var cells: std.ArrayListUnmanaged(ArborGhosttyStyledCell) = .empty;
    defer cells.deinit(state.alloc);

    var text: std.ArrayListUnmanaged(u8) = .empty;
    defer text.deinit(state.alloc);

    var page_it = state.terminal.screens.active.pages.pageIterator(
        .right_down,
        lib.Point{ .screen = .{} },
        null,
    );

    while (page_it.next()) |chunk| {
        const page = &chunk.node.data;
        const rows = chunk.rows();

        for (rows) |*row| {
            const line_cell_start = cells.items.len;
            const row_cells = page.getCells(row);

            for (row_cells, 0..) |*cell, column| {
                switch (cell.wide) {
                    .spacer_head, .spacer_tail => continue,
                    .narrow, .wide => {},
                }

                const text_offset = text.items.len;
                switch (cell.content_tag) {
                    .bg_color_palette, .bg_color_rgb => text.append(state.alloc, ' ') catch return 2,
                    .codepoint, .codepoint_grapheme => {
                        const cp = cell.codepoint();
                        if (cp == 0) {
                            text.append(state.alloc, ' ') catch return 2;
                        } else {
                            encodeCodepoint(&text, state.alloc, cp) catch return 2;
                            if (cell.hasGrapheme()) {
                                if (page.lookupGrapheme(cell)) |grapheme| {
                                    for (grapheme) |extra_cp| {
                                        encodeCodepoint(&text, state.alloc, extra_cp) catch return 2;
                                    }
                                }
                            }
                        }
                    },
                }

                const style = if (cell.style_id == 0)
                    lib.Style{}
                else
                    page.styles.get(page.memory, cell.style_id).*;
                const colors = resolveColors(state, cell, style);

                cells.append(state.alloc, .{
                    .column = column,
                    .text_offset = text_offset,
                    .text_len = text.items.len - text_offset,
                    .fg = colors.fg,
                    .bg = colors.bg,
                }) catch return 2;
            }

            lines.append(state.alloc, .{
                .cell_start = line_cell_start,
                .cell_len = cells.items.len - line_cell_start,
            }) catch return 2;
        }
    }

    const owned_lines = state.alloc.dupe(ArborGhosttyStyledLine, lines.items) catch return 2;
    errdefer if (owned_lines.len > 0) state.alloc.free(owned_lines);

    const owned_cells = state.alloc.dupe(ArborGhosttyStyledCell, cells.items) catch return 2;
    errdefer if (owned_cells.len > 0) state.alloc.free(owned_cells);

    const owned_text = state.alloc.dupe(u8, text.items) catch return 2;
    errdefer if (owned_text.len > 0) state.alloc.free(owned_text);

    out.* = .{
        .lines_ptr = owned_lines.ptr,
        .lines_len = owned_lines.len,
        .cells_ptr = owned_cells.ptr,
        .cells_len = owned_cells.len,
        .text_ptr = owned_text.ptr,
        .text_len = owned_text.len,
        .cursor_visible = cursor.visible,
        .cursor_line = cursor.line,
        .cursor_column = cursor.column,
        .app_cursor = state.terminal.modes.get(.cursor_keys),
        .alt_screen = state.terminal.screens.active_key == .alternate,
    };
    return 0;
}

pub export fn arbor_ghostty_vt_snapshot_cursor(
    handle: ?*anyopaque,
    visible: *bool,
    line: *usize,
    column: *usize,
) i32 {
    const state = ptrFromHandle(handle) orelse return 1;
    const cursor = currentScreenCursor(state);
    visible.* = cursor.visible;
    line.* = cursor.line;
    column.* = cursor.column;
    return 0;
}

pub export fn arbor_ghostty_vt_snapshot_modes(
    handle: ?*anyopaque,
    app_cursor: *bool,
    alt_screen: *bool,
) i32 {
    const state = ptrFromHandle(handle) orelse return 1;
    app_cursor.* = state.terminal.modes.get(.cursor_keys);
    alt_screen.* = state.terminal.screens.active_key == .alternate;
    return 0;
}

pub export fn arbor_ghostty_vt_free_buffer(buffer: ArborGhosttyBuffer) void {
    if (buffer.len == 0) return;
    page_allocator.free(buffer.ptr[0..buffer.len]);
}

pub export fn arbor_ghostty_vt_free_styled_snapshot(snapshot: ArborGhosttyStyledSnapshot) void {
    if (snapshot.lines_len > 0) page_allocator.free(snapshot.lines_ptr[0..snapshot.lines_len]);
    if (snapshot.cells_len > 0) page_allocator.free(snapshot.cells_ptr[0..snapshot.cells_len]);
    if (snapshot.text_len > 0) page_allocator.free(snapshot.text_ptr[0..snapshot.text_len]);
}
