//! Runtime configuration, read from environment variables.

use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub listen: SocketAddr,
    pub game_port_min: u16,
    pub game_port_max: u16,
    pub rcon_port_base: u16,
    pub tls_sans: Vec<String>,
    /// Java version label -> path of the `java` executable.
    pub javas: BTreeMap<String, PathBuf>,
    pub default_java: String,
    pub boot_timeout: Duration,
    pub stop_timeout: Duration,
    pub autostart_delay: Duration,
    pub max_upload_bytes: u64,
    /// The operator agreed to the Minecraft EULA during setup (setup.sh); instances
    /// are then created with `eula=true` automatically.
    pub eula_accepted: bool,
    pub eula_accepted_at: String,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_secs(key: &str, default: u64) -> Result<Duration> {
    let v = env_or(key, &default.to_string());
    Ok(Duration::from_secs(
        v.parse().with_context(|| format!("{key} must be a number of seconds"))?,
    ))
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let data_dir = PathBuf::from(env_or("ABMPT_DATA_DIR", "/data"));
        let listen: SocketAddr = env_or("ABMPT_LISTEN", "0.0.0.0:3307")
            .parse()
            .context("ABMPT_LISTEN must be host:port")?;

        let ports = env_or("ABMPT_GAME_PORTS", "65000-65100");
        let (lo, hi) = ports
            .split_once('-')
            .context("ABMPT_GAME_PORTS must look like 65000-65100")?;
        let game_port_min: u16 = lo.trim().parse().context("invalid ABMPT_GAME_PORTS")?;
        let game_port_max: u16 = hi.trim().parse().context("invalid ABMPT_GAME_PORTS")?;
        if game_port_min > game_port_max {
            bail!("ABMPT_GAME_PORTS range is empty");
        }
        let rcon_port_base: u16 = env_or("ABMPT_RCON_PORT_BASE", "35000")
            .parse()
            .context("invalid ABMPT_RCON_PORT_BASE")?;
        let span = u32::from(game_port_max - game_port_min);
        if u32::from(rcon_port_base) + span > u32::from(u16::MAX) {
            bail!("ABMPT_RCON_PORT_BASE too high for the game port range");
        }
        let rcon_hi = rcon_port_base as u32 + span;
        if (rcon_port_base as u32) <= game_port_max as u32 && rcon_hi >= game_port_min as u32 {
            bail!("RCON port range overlaps the game port range");
        }

        let tls_sans: Vec<String> = env_or("ABMPT_TLS_SANS", "localhost,127.0.0.1")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // Java runtimes: ABMPT_JAVA_<label>=/path/to/java. Keeps room for more versions later.
        let mut javas = BTreeMap::new();
        for (k, v) in std::env::vars() {
            if let Some(label) = k.strip_prefix("ABMPT_JAVA_")
                && !label.is_empty()
                && label != "DEFAULT"
            {
                javas.insert(label.to_string(), PathBuf::from(v));
            }
        }
        if javas.is_empty() {
            javas.insert("25".to_string(), PathBuf::from("java"));
        }
        let default_java = env_or("ABMPT_JAVA_DEFAULT", "25");
        if !javas.contains_key(&default_java) {
            bail!("ABMPT_JAVA_DEFAULT={default_java} has no matching ABMPT_JAVA_{default_java}");
        }

        let max_upload_mb: u64 = env_or("ABMPT_MAX_UPLOAD_MB", "512")
            .parse()
            .context("invalid ABMPT_MAX_UPLOAD_MB")?;

        Ok(Self {
            data_dir,
            listen,
            game_port_min,
            game_port_max,
            rcon_port_base,
            tls_sans,
            javas,
            default_java,
            boot_timeout: env_secs("ABMPT_BOOT_TIMEOUT_SECS", 600)?,
            stop_timeout: env_secs("ABMPT_STOP_TIMEOUT_SECS", 90)?,
            autostart_delay: env_secs("ABMPT_AUTOSTART_DELAY_SECS", 5)?,
            max_upload_bytes: max_upload_mb * 1024 * 1024,
            eula_accepted: env_or("ABMPT_EULA_ACCEPTED", "false").eq_ignore_ascii_case("true"),
            eula_accepted_at: env_or("ABMPT_EULA_ACCEPTED_AT", "unknown time"),
        })
    }

    pub fn rcon_port_for(&self, game_port: u16) -> u16 {
        self.rcon_port_base + (game_port - self.game_port_min)
    }

    pub fn paths(&self) -> DataPaths {
        DataPaths {
            root: self.data_dir.clone(),
        }
    }
}

/// Layout of the persistent data directory (the Docker volume).
#[derive(Debug, Clone)]
pub struct DataPaths {
    pub root: PathBuf,
}

impl DataPaths {
    pub fn tokens_file(&self) -> PathBuf {
        self.root.join("tokens.json")
    }
    pub fn tls_dir(&self) -> PathBuf {
        self.root.join("tls")
    }
    pub fn jars_dir(&self) -> PathBuf {
        self.root.join("jars")
    }
    pub fn uploads_dir(&self) -> PathBuf {
        self.root.join("uploads")
    }
    pub fn meta_dir(&self) -> PathBuf {
        self.root.join("meta")
    }
    pub fn instances_dir(&self) -> PathBuf {
        self.root.join("instances")
    }
    pub fn instance_dir(&self, name: &str) -> PathBuf {
        self.instances_dir().join(name)
    }
    pub fn backups_dir(&self, name: &str) -> PathBuf {
        self.root.join("backups").join(name)
    }
    pub fn audit_file(&self) -> PathBuf {
        self.root.join("audit").join("audit.jsonl")
    }

    pub fn ensure(&self) -> Result<()> {
        for d in [
            self.tls_dir(),
            self.jars_dir(),
            self.uploads_dir(),
            self.meta_dir(),
            self.instances_dir(),
            self.root.join("backups"),
            self.root.join("audit"),
        ] {
            std::fs::create_dir_all(&d).with_context(|| format!("create {}", d.display()))?;
        }
        Ok(())
    }
}
