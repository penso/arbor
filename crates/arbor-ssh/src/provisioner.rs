use {
    crate::connection::SshConnection,
    arbor_core::{
        outpost::RemoteHost,
        remote::{ProvisionResult, RemoteError, RemoteProvisioner, RemoteTransport},
    },
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
            // Clone the default branch first, then create the new branch.
            // Using --branch would fail if the branch doesn't exist on the
            // remote yet (which is the common case for new outposts).
            let clone_cmd = format!(
                "GIT_SSH_COMMAND='ssh -F /dev/null' git clone {clone_url} {remote_path}"
            );
            tracing::info!(
                clone_url,
                branch,
                remote_path,
                "cloning repository on remote host"
            );
            #[cfg(unix)]
            let clone_output = self
                .connection
                .run_command_with_agent_forwarding(&clone_cmd)?;
            #[cfg(not(unix))]
            let clone_output = self.connection.run_command(&clone_cmd)?;
            if clone_output.exit_code != Some(0) {
                tracing::error!(
                    clone_url,
                    branch,
                    remote_path,
                    stderr = clone_output.stderr.as_str(),
                    "git clone failed on remote host"
                );
                return Err(RemoteError::Command(format!(
                    "git clone failed: {}",
                    clone_output.stderr,
                )));
            }
        }

        // Create and switch to the target branch.  If it already exists
        // (e.g. the repo was already cloned), just check it out.
        let checkout_cmd = format!(
            "cd {remote_path} && \
             git checkout {branch} 2>/dev/null || git checkout -b {branch}"
        );
        let checkout_output = self.connection.run_command(&checkout_cmd)?;
        if checkout_output.exit_code != Some(0) {
            tracing::error!(
                branch,
                remote_path,
                stderr = checkout_output.stderr.as_str(),
                "branch checkout failed on remote host"
            );
            return Err(RemoteError::Command(format!(
                "branch checkout failed: {}",
                checkout_output.stderr,
            )));
        }

        let has_remote_daemon = detect_remote_daemon(self.connection, self.host);

        Ok(ProvisionResult {
            remote_path,
            has_remote_daemon,
        })
    }
}

fn detect_remote_daemon(connection: &SshConnection, host: &RemoteHost) -> bool {
    let Some(daemon_port) = host.daemon_port else {
        return false;
    };

    let check_cmd =
        format!("curl -sf http://127.0.0.1:{daemon_port}/api/sessions > /dev/null 2>&1 && echo ok");
    match connection.run_command(&check_cmd) {
        Ok(output) => output.stdout.trim() == "ok",
        Err(_) => false,
    }
}
