use {arbor_core::remote::RemoteTransport, std::process::Command};

pub fn local_mosh_client_available() -> bool {
    Command::new("which")
        .arg("mosh-client")
        .output()
        .is_ok_and(|output| output.status.success())
}

pub fn remote_mosh_server_available(connection: &dyn RemoteTransport) -> bool {
    connection
        .run_command("which mosh-server")
        .is_ok_and(|output| output.exit_code == Some(0))
}
