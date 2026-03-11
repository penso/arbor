const std = @import("std");
const lib = @import("ghostty-vt");

const Allocator = std.mem.Allocator;
const page_allocator = std.heap.page_allocator;

const State = struct {
    alloc: Allocator,
    terminal: lib.Terminal,
    stream: lib.ReadonlyStream,
};

pub const ArborGhosttyBuffer = extern struct {
    ptr: [*]u8,
    len: usize,
};

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

    state.stream = state.terminal.vtStream();
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
) i32 {
    const state = ptrFromHandle(handle) orelse return 1;
    if (len == 0) return 0;
    state.stream.nextSlice(bytes[0..len]) catch return 2;
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

pub export fn arbor_ghostty_vt_snapshot_cursor(
    handle: ?*anyopaque,
    visible: *bool,
    line: *usize,
    column: *usize,
) i32 {
    const state = ptrFromHandle(handle) orelse return 1;
    visible.* = state.terminal.modes.get(.cursor_visible);
    line.* = state.terminal.screens.active.cursor.y;
    column.* = state.terminal.screens.active.cursor.x;
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
