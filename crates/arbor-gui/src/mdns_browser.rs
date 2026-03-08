use mdns_sd::{ServiceDaemon, ServiceEvent};

const SERVICE_TYPE: &str = "_arbor._tcp.local.";

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredDaemon {
    pub instance_name: String,
    pub host: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub tls: bool,
    pub has_auth: bool,
    pub version: String,
}

impl DiscoveredDaemon {
    pub fn base_url(&self) -> String {
        let scheme = if self.tls {
            "https"
        } else {
            "http"
        };
        let host = self
            .addresses
            .first()
            .cloned()
            .unwrap_or_else(|| self.host.clone());
        format!("{scheme}://{host}:{}", self.port)
    }

    /// Short display label — hostname without `.local.` suffix.
    pub fn display_name(&self) -> &str {
        self.instance_name
            .strip_suffix(".local.")
            .unwrap_or(&self.instance_name)
    }
}

pub enum MdnsEvent {
    Added(DiscoveredDaemon),
    Removed(String),
}

pub trait MdnsDiscovery {
    fn poll_updates(&self) -> Vec<MdnsEvent>;
}

struct MdnsSdBrowser {
    _daemon: ServiceDaemon,
    receiver: mdns_sd::Receiver<ServiceEvent>,
}

/// Start browsing for `_arbor._tcp` services on the local network.
pub fn start_browsing() -> Result<Box<dyn MdnsDiscovery>, String> {
    let daemon = ServiceDaemon::new().map_err(|e| format!("failed to create mDNS daemon: {e}"))?;
    let receiver = daemon
        .browse(SERVICE_TYPE)
        .map_err(|e| format!("failed to browse for {SERVICE_TYPE}: {e}"))?;
    Ok(Box::new(MdnsSdBrowser {
        _daemon: daemon,
        receiver,
    }))
}

impl MdnsDiscovery for MdnsSdBrowser {
    /// Non-blocking drain of pending mDNS events.
    fn poll_updates(&self) -> Vec<MdnsEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    let tls = info
                        .get_property_val_str("tls")
                        .is_some_and(|v| v == "true");
                    let has_auth = info
                        .get_property_val_str("auth")
                        .is_some_and(|v| v == "true");
                    let version = info
                        .get_property_val_str("version")
                        .unwrap_or_default()
                        .to_owned();

                    let addresses: Vec<String> =
                        info.get_addresses().iter().map(|a| a.to_string()).collect();

                    let daemon = DiscoveredDaemon {
                        instance_name: info.get_fullname().to_owned(),
                        host: info.get_hostname().to_owned(),
                        addresses,
                        port: info.get_port(),
                        tls,
                        has_auth,
                        version,
                    };
                    events.push(MdnsEvent::Added(daemon));
                },
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    events.push(MdnsEvent::Removed(fullname));
                },
                _ => {},
            }
        }
        events
    }
}
