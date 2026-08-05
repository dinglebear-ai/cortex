use std::env;
use std::path::PathBuf;
use std::time::Duration;

use crate::inventory::limits::{
    DEFAULT_COLLECTION_DEADLINE_SECS, DEFAULT_COLLECTOR_DEADLINE_SECS, DEFAULT_PROBE_DEADLINE_SECS,
};

#[derive(Clone)]
pub struct InventoryConfig {
    pub root: PathBuf,
    pub compose_paths: Vec<PathBuf>,
    pub proxy_paths: Vec<PathBuf>,
    pub adguard_paths: Vec<PathBuf>,
    pub ssh_config: Option<PathBuf>,
    pub ssh_hosts: Vec<String>,
    pub project_roots: Vec<PathBuf>,
    pub docker_hosts: Vec<String>,
    pub unraid_url: Option<String>,
    pub unraid_api_key: Option<String>,
    pub unifi_url: Option<String>,
    pub unifi_api_key: Option<String>,
    pub media_services: Vec<MediaServiceConfig>,
    pub collection_deadline: Duration,
    pub collector_deadline: Duration,
    pub probe_deadline: Duration,
}

#[derive(Clone)]
pub struct MediaServiceConfig {
    pub kind: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl InventoryConfig {
    pub fn from_env() -> Self {
        let cortex_home = env::var_os("CORTEX_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cortex")))
            .unwrap_or_else(|| PathBuf::from(".cortex"));
        let root =
            env_path("CORTEX_INVENTORY_DIR").unwrap_or_else(|| cortex_home.join("inventory"));
        Self {
            root,
            compose_paths: env_paths("CORTEX_INVENTORY_COMPOSE_PATHS")
                .unwrap_or_else(|| vec![cortex_home.join("compose/docker-compose.yml")]),
            proxy_paths: env_paths("CORTEX_INVENTORY_PROXY_PATHS").unwrap_or_default(),
            adguard_paths: env_paths("CORTEX_INVENTORY_ADGUARD_PATHS")
                .unwrap_or_else(default_adguard_paths),
            ssh_config: env_path("CORTEX_INVENTORY_SSH_CONFIG").or_else(|| {
                env::var_os("HOME").map(|home| PathBuf::from(home).join(".ssh/config"))
            }),
            ssh_hosts: split_env("CORTEX_INVENTORY_SSH_HOSTS"),
            project_roots: env_paths("CORTEX_INVENTORY_PROJECT_ROOTS").unwrap_or_else(|| {
                env::var_os("HOME")
                    .map(|home| vec![PathBuf::from(home).join("workspace")])
                    .unwrap_or_default()
            }),
            docker_hosts: split_env("CORTEX_DOCKER_HOSTS")
                .into_iter()
                .map(|host| {
                    if host.starts_with("http://") || host.starts_with("https://") {
                        host
                    } else {
                        format!("http://{host}:2375")
                    }
                })
                .collect(),
            unraid_url: env_string("CORTEX_UNRAID_URL"),
            unraid_api_key: env_string("CORTEX_UNRAID_API_KEY"),
            unifi_url: env_string("CORTEX_UNIFI_URL"),
            unifi_api_key: env_string("CORTEX_UNIFI_API_KEY"),
            media_services: media_services_from_env(),
            collection_deadline: env_secs(
                "CORTEX_INVENTORY_COLLECTION_DEADLINE_SECS",
                DEFAULT_COLLECTION_DEADLINE_SECS,
            ),
            collector_deadline: env_secs(
                "CORTEX_INVENTORY_COLLECTOR_DEADLINE_SECS",
                DEFAULT_COLLECTOR_DEADLINE_SECS,
            ),
            probe_deadline: env_secs(
                "CORTEX_INVENTORY_PROBE_DEADLINE_SECS",
                DEFAULT_PROBE_DEADLINE_SECS,
            ),
        }
    }
}

fn default_adguard_paths() -> Vec<PathBuf> {
    [
        "/mnt/appdata/adguard/etc/config.yaml",
        "/mnt/cache/appdata/adguard/etc/config.yaml",
        "/mnt/user/appdata/adguard/etc/config.yaml",
        "/opt/AdGuardHome/AdGuardHome.yaml",
        "/opt/adguardhome/conf/AdGuardHome.yaml",
        "/etc/AdGuardHome.yaml",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

fn env_string(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_path(name: &str) -> Option<PathBuf> {
    env_string(name).map(PathBuf::from)
}

fn env_paths(name: &str) -> Option<Vec<PathBuf>> {
    let paths = split_env(name)
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    (!paths.is_empty()).then_some(paths)
}

fn split_env(name: &str) -> Vec<String> {
    env_string(name)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn env_secs(name: &str, default: u64) -> Duration {
    env_string(name)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(default))
}

fn media_services_from_env() -> Vec<MediaServiceConfig> {
    let mut out = Vec::new();
    for kind in [
        "sonarr",
        "radarr",
        "prowlarr",
        "sabnzbd",
        "qbittorrent",
        "plex",
        "tautulli",
        "overseerr",
    ] {
        let upper = kind.to_ascii_uppercase();
        if let Some(base_url) = env_string(&format!("CORTEX_{}_URL", upper)) {
            out.push(MediaServiceConfig {
                kind: kind.to_string(),
                base_url,
                api_key: env_string(&format!("CORTEX_{}_API_KEY", upper))
                    .or_else(|| env_string(&format!("CORTEX_{}_TOKEN", upper))),
                username: env_string(&format!("CORTEX_{}_USERNAME", upper)),
                password: env_string(&format!("CORTEX_{}_PASSWORD", upper)),
            });
        }
    }
    out
}
