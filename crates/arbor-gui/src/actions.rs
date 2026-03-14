use gpui::actions;

actions!(arbor, [
    ShowAbout,
    RequestQuit,
    ImmediateQuit,
    NewWindow,
    SpawnTerminal,
    CloseActiveTerminal,
    OpenManagePresets,
    OpenManageRepoPresets,
    RefreshWorktrees,
    RefreshChanges,
    OpenAddRepository,
    OpenCreateWorktree,
    ToggleLeftPane,
    NavigateWorktreeBack,
    NavigateWorktreeForward,
    CollapseAllRepositories,
    ViewLogs,
    OpenCommandPalette,
    OpenThemePicker,
    OpenSettings,
    OpenManageHosts,
    ConnectToHost,
    RefreshReviewComments
]);

#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = arbor, no_json)]
pub struct ConnectToLanDaemon {
    pub index: usize,
}
