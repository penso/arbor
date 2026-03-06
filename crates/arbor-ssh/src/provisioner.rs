use {
    arbor_core::{
        outpost::RemoteHost,
        remote::{
            ProvisionResult, RemoteCommandOutput, RemoteError, RemoteProvisioner, RemoteTransport,
        },
    },
    crate::connection::SshConnection,
    std::process::Command,
};

pub struct SshProvisioner<'a> {
    connection: &'a SshConnection,
    host: &'a RemoteHost,
}

impl<'a> SshProvisioner<'a> {
    pub fn new(connection: &'a SshConnection, host: &'a RemoteHost) -> Self {
        Self { connection, host }
    }
}

impl RemoteProvisioner for SshProvisioner<'_> {
    fn provision(
        &self,
        clone_url: &str,
        outpost_label: &str,
        branch: &str,
    ) -> Result<ProvisionResult, RemoteError> {
        let base_path = &self.host.remote_base_path;
        let remote_path = format!("{base_path}/{outpost_label}");

        let mkdir_output = self
            .connection
            .run_command(&format!("mkdir -p {remote_path}"))?;
        if mkdir_output.exit_code != Some(0) {
            return Err(RemoteError::Command(format!(
                "failed to create remote directory: {}",
                mkdir_output.stderr,
            )));
        }

        let check_output = self
            .connection
            .run_command(&format!("test -d {remote_path}/.git && echo exists"))?;
        let already_cloned = check_output.stdout.trim() == "exists";

        if !already_cloned {
            let clone_cmd =
                format!("git clone --branch {branch} --single-branch {clone_url} {remote_path}");
            let clone_output = run_ssh_command_with_agent(self.host, &clone_cmd)?;
            if clone_output.exit_code != Some(0) {
                return Err(RemoteError::Command(format!(
                    "git clone failed: {}",
                    clone_output.stderr,
                )));
            }
        }

        let has_remote_daemon = detect_remote_daemon(self.connection, self.host);

        Ok(ProvisionResult {
            remote_path,
            has_remote_daemon,
        })
    }
}

fn run_ssh_command_with_agent(
    host: &RemoteHost,
    command: &str,
) -> Result<RemoteCommandOutput, RemoteError> {
    let mut cmd = Command::new("ssh");
    cmd.arg("-A")
        .arg("-o").arg("BatchMode=yes")
        .arg("-o").arg("StrictHostKeyChecking=accept-new");

    if host.port != 22 {
        cmd.arg("-p").arg(host.port.to_string());
    }

    if let Some(ref identity_file) = host.identity_file {
        cmd.arg("-i").arg(identity_file);
    }

    cmd.arg(format!("{}@{}", host.user, host.hostname))
        .arg(command);

    let output = cmd
        .output()
        .map_err(|e| RemoteError::Command(format!("failed to spawn ssh: {e}")))?;

    Ok(RemoteCommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code(),
    })
}

fn detect_remote_daemon(connection: &SshConnection, host: &RemoteHost) -> bool {
    let Some(daemon_port) = host.daemon_port else {
        return false;
    };

    let check_cmd = format!(
        "curl -sf http://127.0.0.1:{daemon_port}/api/sessions > /dev/null 2>&1 && echo ok"
    );
    match connection.run_command(&check_cmd) {
        Ok(output) => output.stdout.trim() == "ok",
        Err(_) => false,
    }
}
