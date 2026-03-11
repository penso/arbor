const std = @import("std");
const buildpkg = @import("src/build/main.zig");
const appVersion = @import("build.zig.zon").version;
const minimumZigVersion = @import("build.zig.zon").minimum_zig_version;

comptime {
    buildpkg.requireZig(minimumZigVersion);
}

pub fn build(b: *std.Build) void {
    const config = buildpkg.Config.init(b, appVersion) catch unreachable;
    const deps = buildpkg.SharedDeps.init(b, &config) catch unreachable;
    const ghostty_zig = buildpkg.GhosttyZig.init(b, &config, &deps) catch unreachable;

    const root = b.createModule(.{
        .root_source_file = b.path("arbor_bridge.zig"),
        .target = config.target,
        .optimize = config.optimize,
    });
    root.addImport("ghostty-vt", ghostty_zig.vt);

    const lib = b.addLibrary(.{
        .name = "arbor_ghostty_vt_bridge",
        .linkage = .dynamic,
        .root_module = root,
    });
    b.installArtifact(lib);
}
