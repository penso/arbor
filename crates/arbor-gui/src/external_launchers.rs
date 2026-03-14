fn open_worktree_in_file_manager(worktree_path: &Path) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let mut command = create_command("open");
        command.arg(worktree_path);
        run_launch_command(&mut command, "open worktree in Finder")?;
        return Ok("opened worktree in Finder".to_owned());
    }

    #[cfg(target_os = "linux")]
    {
        let mut command = create_command("xdg-open");
        command.arg(worktree_path);
        run_launch_command(&mut command, "open worktree in file manager")?;
        return Ok("opened worktree in file manager".to_owned());
    }

    #[cfg(target_os = "windows")]
    {
        let mut command = create_command("explorer");
        command.arg(worktree_path);
        run_launch_command(&mut command, "open worktree in File Explorer")?;
        return Ok("opened worktree in File Explorer".to_owned());
    }

    #[allow(unreachable_code)]
    Err("opening this worktree in a file manager is not supported on this platform".to_owned())
}

fn open_worktree_with_external_launcher(
    worktree_path: &Path,
    launcher: ExternalLauncher,
) -> Result<String, String> {
    match launcher.kind {
        ExternalLauncherKind::Command(command_name) => {
            let mut command = create_command(command_name);
            command.arg(worktree_path);
            run_launch_command(
                &mut command,
                &format!("open worktree with {}", launcher.label),
            )?;
        },
        ExternalLauncherKind::MacApp(app_name) => {
            let mut command = create_command("open");
            command.arg("-a").arg(app_name).arg(worktree_path);
            run_launch_command(
                &mut command,
                &format!("open worktree in {}", launcher.label),
            )?;
        },
    }

    Ok(format!("opened worktree in {}", launcher.label))
}

fn command_exists_on_path(command_name: &str) -> bool {
    let path_env = AUGMENTED_PATH
        .get()
        .map(|path| std::ffi::OsString::from(path.as_str()))
        .or_else(|| env::var_os("PATH"));

    let Some(path_env) = path_env else {
        return false;
    };

    env::split_paths(&path_env).any(|directory| directory.join(command_name).is_file())
}

#[cfg(target_os = "macos")]
fn mac_app_bundle_exists(app_name: &str) -> bool {
    let bundle = format!("{app_name}.app");
    [
        "/Applications",
        "/System/Applications",
        "/System/Applications/Utilities",
    ]
    .iter()
    .map(PathBuf::from)
    .chain(
        env::var_os("HOME")
            .map(PathBuf::from)
            .into_iter()
            .map(|home| home.join("Applications")),
    )
    .any(|base| base.join(&bundle).exists())
}

#[cfg(not(target_os = "macos"))]
fn mac_app_bundle_exists(_: &str) -> bool {
    false
}

fn detect_external_launcher(
    label: &'static str,
    icon: &'static str,
    icon_color: u32,
    mac_app: Option<&'static str>,
    command: Option<&'static str>,
) -> Option<ExternalLauncher> {
    if let Some(app_name) = mac_app
        && mac_app_bundle_exists(app_name)
    {
        return Some(ExternalLauncher {
            label,
            icon,
            icon_color,
            kind: ExternalLauncherKind::MacApp(app_name),
        });
    }

    if let Some(command_name) = command
        && command_exists_on_path(command_name)
    {
        return Some(ExternalLauncher {
            label,
            icon,
            icon_color,
            kind: ExternalLauncherKind::Command(command_name),
        });
    }

    None
}

fn detect_ide_launchers() -> Vec<ExternalLauncher> {
    [
        (
            "VS Code",
            "\u{e70c}",
            0x2f80ed,
            Some("Visual Studio Code"),
            Some("code"),
        ),
        (
            "VS Code Insiders",
            "\u{e70c}",
            0x4f9fff,
            Some("Visual Studio Code - Insiders"),
            Some("code-insiders"),
        ),
        ("Cursor", "Cu", 0x6ca6ff, Some("Cursor"), Some("cursor")),
        ("Zed", "Ze", 0x59a6ff, Some("Zed"), Some("zed")),
        (
            "Windsurf",
            "Ws",
            0x3cb9fc,
            Some("Windsurf"),
            Some("windsurf"),
        ),
        ("VSCodium", "Vc", 0x23a8f2, Some("VSCodium"), Some("codium")),
    ]
    .into_iter()
    .filter_map(|(label, icon, icon_color, mac_app, command)| {
        detect_external_launcher(label, icon, icon_color, mac_app, command)
    })
    .collect()
}
