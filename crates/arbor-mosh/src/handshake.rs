use {
    arbor_core::{outpost::RemoteHost, remote::RemoteTransport},
    crate::MoshError,
};

#[derive(Debug, Clone)]
pub struct MoshHandshakeResult {
    pub port: u16,
    pub key: String,
    pub hostname: String,
}

pub fn start_mosh_server(
    connection: &dyn RemoteTransport,
    host: &RemoteHost,
) -> Result<MoshHandshakeResult, MoshError> {
    let server_binary = host
        .mosh_server_path
        .as_deref()
        .unwrap_or("mosh-server");

    let command = format!("{server_binary} new -s");

    let output = connection
        .run_command(&command)
        .map_err(|error| MoshError::Ssh(error.to_string()))?;

    if output.exit_code != Some(0) {
        let stderr = output.stderr.trim();
        if stderr.contains("not found") || output.exit_code == Some(127) {
            return Err(MoshError::ServerNotInstalled(host.hostname.clone()));
        }
        return Err(MoshError::ServerStartFailed(format!(
            "exit code {:?}: {}",
            output.exit_code,
            stderr,
        )));
    }

    parse_mosh_connect(&output.stdout, &host.hostname)
}

fn parse_mosh_connect(stdout: &str, hostname: &str) -> Result<MoshHandshakeResult, MoshError> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("MOSH CONNECT") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 2 {
                let port: u16 = parts[0].parse().map_err(|error| {
                    MoshError::HandshakeParseFailed(format!(
                        "invalid port `{}`: {error}",
                        parts[0],
                    ))
                })?;
                let key = parts[1].to_owned();
                return Ok(MoshHandshakeResult {
                    port,
                    key,
                    hostname: hostname.to_owned(),
                });
            }
        }
    }

    Err(MoshError::HandshakeParseFailed(
        "no MOSH CONNECT line found in mosh-server output".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_mosh_connect_line() {
        let stdout = "MOSH CONNECT 60001 AbCdEfGh12345678901234\n";
        let result = parse_mosh_connect(stdout, "build.example.com");
        let handshake = result.expect("should parse");
        assert_eq!(handshake.port, 60001);
        assert_eq!(handshake.key, "AbCdEfGh12345678901234");
        assert_eq!(handshake.hostname, "build.example.com");
    }

    #[test]
    fn parses_mosh_connect_with_preamble() {
        let stdout = "\n\
            mosh-server (mosh 1.4.0) [build mosh-1.4.0]\n\
            Copyright 2012 Keith Winstein <mosh-devel@mit.edu>\n\
            License GPLv3+: GNU GPL version 3 or later\n\
            \n\
            [mosh-server detached, pid = 12345]\n\
            \n\
            MOSH CONNECT 60002 ZzYyXxWwVvUu99887766\n";
        let result = parse_mosh_connect(stdout, "server.local");
        let handshake = result.expect("should parse with preamble");
        assert_eq!(handshake.port, 60002);
        assert_eq!(handshake.key, "ZzYyXxWwVvUu99887766");
    }

    #[test]
    fn fails_on_missing_connect_line() {
        let stdout = "some random output\nno connect here\n";
        let result = parse_mosh_connect(stdout, "host");
        assert!(result.is_err());
    }

    #[test]
    fn fails_on_invalid_port() {
        let stdout = "MOSH CONNECT notaport SomeKey123\n";
        let result = parse_mosh_connect(stdout, "host");
        assert!(result.is_err());
    }

    #[test]
    fn parses_with_leading_whitespace() {
        let stdout = "  MOSH CONNECT 60003 KeyWithSpaces123  \n";
        let result = parse_mosh_connect(stdout, "host");
        let handshake = result.expect("should handle leading/trailing whitespace");
        assert_eq!(handshake.port, 60003);
        assert_eq!(handshake.key, "KeyWithSpaces123");
    }
}
