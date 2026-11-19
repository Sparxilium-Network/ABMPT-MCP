//! Instance registry, lifecycle (create / init / start / stop / delete) and locking.

mod files;
mod plugins;
mod process;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex, RwLock},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use rand::RngCore;
use regex::Regex;
use serde::{Deserialize, Serialize};

use process::{ProcHandle, SpawnSpec};

use crate::{
    config::{Config, DataPaths},
    props,
    redact::Redactor,
    store::{CoreJars, Uploads},
    util::{self, validate_name},
};

/// Keys in server.properties owned by the MCP service.
pub const MANAGED_PROPS: &[&str] = &[
    "server-port",
    "server-ip",
    "enable-rcon",
    "rcon.port",
    "rcon.password",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceMeta {
    pub name: String,
    pub core_jar: String,
    pub memory: String,
    pub autostart: bool,
    pub game_port: u16,
    pub rcon_port: u16,
    /// Never returned to callers.
    pub rcon_password: String,
    pub java: String,
    pub initialized: bool,
    pub created_at: String,
    pub created_by: String,
    pub eula_accepted_by: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Created but never initialized.
    Created,
    Stopped,
    Initializing,
    Starting,
    Running,
    Stopping,
}

struct Runtime {
    status: Status,
    proc: Option<Arc<ProcHandle>>,
    generation: u64,
    last_exit: Option<String>,
}

#[derive(Debug, Clone)]
struct BusyInfo {
    agent: String,
    op: String,
    since: String,
}

pub struct Instance {
    pub name: String,
    meta: Mutex<InstanceMeta>,
    rt: Mutex<Runtime>,
    busy: Mutex<Option<BusyInfo>>,
}

/// Held while a state-changing operation runs on an instance.
pub struct BusyGuard {
    inst: Arc<Instance>,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        *self.inst.busy.lock().unwrap() = None;
    }
}

impl Instance {
    fn new(meta: InstanceMeta) -> Self {
        let status = if meta.initialized {
            Status::Stopped
        } else {
            Status::Created
        };
        Self {
            name: meta.name.clone(),
            meta: Mutex::new(meta),
            rt: Mutex::new(Runtime {
                status,
                proc: None,
                generation: 0,
                last_exit: None,
            }),
            busy: Mutex::new(None),
        }
    }

    pub fn meta(&self) -> InstanceMeta {
        self.meta.lock().unwrap().clone()
    }

    pub fn status(&self) -> Status {
        self.rt.lock().unwrap().status
    }

    fn set_status(&self, s: Status) {
        self.rt.lock().unwrap().status = s;
    }

    fn idle_status(&self) -> Status {
        if self.meta.lock().unwrap().initialized {
            Status::Stopped
        } else {
            Status::Created
        }
    }

    fn proc(&self) -> Option<Arc<ProcHandle>> {
        self.rt.lock().unwrap().proc.clone()
    }

    pub fn acquire(self: &Arc<Self>, agent: &str, op: &str) -> Result<BusyGuard> {
        let mut b = self.busy.lock().unwrap();
        if let Some(cur) = b.as_ref() {
            bail!(
                "instance '{}' is busy: '{}' requested by agent '{}' is in progress since {}. Try again after it finishes.",
                self.name,
                cur.op,
                cur.agent,
                cur.since
            );
        }
        *b = Some(BusyInfo {
            agent: agent.to_string(),
            op: op.to_string(),
            since: util::now_rfc3339(),
        });
        Ok(BusyGuard { inst: self.clone() })
    }

    fn busy_info(&self) -> Option<String> {
        self.busy
            .lock()
            .unwrap()
            .as_ref()
            .map(|b| format!("{} by {} since {}", b.op, b.agent, b.since))
    }
}

#[derive(Debug, Serialize)]
pub struct BootReport {
    pub ok: bool,
    pub status: Status,
    pub seconds: f64,
    pub game_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    pub error_count: usize,
    pub warning_count: usize,
    /// First error entries seen while booting (stack traces shortened).
    pub errors: Vec<String>,
    /// Last console lines, included only on failure.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub console_tail: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct StopReport {
    pub was_running: bool,
    pub forced_kill: bool,
    pub seconds: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InitReport {
    pub ok: bool,
    pub boot: BootReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutdown: Option<StopReport>,
    pub new_config_files: Vec<String>,
    pub modified_config_files: Vec<String>,
    pub status: Status,
}

#[derive(Debug, Serialize)]
pub struct InstanceSummary {
    pub name: String,
    pub status: Status,
    pub core_jar: String,
    pub game_port: u16,
    pub memory: String,
    pub autostart: bool,
    pub java: String,
    pub online_players: Option<u32>,
    pub plugins: Vec<plugins::InstalledPlugin>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_exit: Option<String>,
}

pub struct Manager {
    pub cfg: Config,
    pub paths: DataPaths,
    pub jars: CoreJars,
    pub uploads: Uploads,
    pub redactor: Redactor,
    instances: RwLock<BTreeMap<String, Arc<Instance>>>,
    /// Serialises server boots so only one instance starts at a time.
    boot_queue: tokio::sync::Mutex<()>,
    /// Serialises create/delete (name + port allocation).
    registry_lock: tokio::sync::Mutex<()>,
}

static MEMORY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([1-9][0-9]{0,5})([MG])$").unwrap());

pub fn normalize_memory(m: &str) -> Result<String> {
    let up = m.trim().to_ascii_uppercase();
    let up = up.trim_end_matches('B').to_string();
    let c = MEMORY_RE
        .captures(&up)
        .with_context(|| format!("invalid memory '{m}': use e.g. 2G or 1536M"))?;
    let n: u64 = c[1].parse()?;
    let mb = if &c[2] == "G" { n * 1024 } else { n };
    if mb < 512 {
        bail!("memory must be at least 512M");
    }
    Ok(up)
}

static PLAYERS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)there are (\d+)|(\d+)\s*(?:/|of a max)").unwrap());

impl Manager {
    pub fn new(cfg: Config, redactor: Redactor) -> Result<Arc<Self>> {
        let paths = cfg.paths();
        paths.ensure()?;
        let mgr = Self {
            jars: CoreJars::new(paths.jars_dir()),
            uploads: Uploads::new(paths.uploads_dir()),
            paths,
            cfg,
            redactor,
            instances: RwLock::new(BTreeMap::new()),
            boot_queue: tokio::sync::Mutex::new(()),
            registry_lock: tokio::sync::Mutex::new(()),
        };
        mgr.load()?;
        Ok(Arc::new(mgr))
    }

    fn meta_path(&self, name: &str) -> PathBuf {
        self.paths.meta_dir().join(format!("{name}.json"))
    }

    fn load(&self) -> Result<()> {
        let mut map = self.instances.write().unwrap();
        for e in std::fs::read_dir(self.paths.meta_dir())?.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "json") {
                continue;
            }
            match std::fs::read(&p)
                .map_err(anyhow::Error::from)
                .and_then(|b| Ok(serde_json::from_slice::<InstanceMeta>(&b)?))
            {
                Ok(m) => {
                    self.redactor.add(&m.rcon_password);
                    map.insert(m.name.clone(), Arc::new(Instance::new(m)));
                }
                Err(err) => tracing::error!("skipping {}: {err:#}", p.display()),
            }
        }
        tracing::info!("loaded {} instance(s)", map.len());
        Ok(())
    }

    fn save_meta(&self, m: &InstanceMeta) -> Result<()> {
        util::write_private(&self.meta_path(&m.name), &serde_json::to_vec_pretty(m)?)
    }

    pub fn get(&self, name: &str) -> Result<Arc<Instance>> {
        self.instances
            .read()
            .unwrap()
            .get(name)
            .cloned()
            .with_context(|| format!("instance '{name}' not found"))
    }

    pub fn dir(&self, name: &str) -> PathBuf {
        self.paths.instance_dir(name)
    }

    pub fn all(&self) -> Vec<Arc<Instance>> {
        self.instances.read().unwrap().values().cloned().collect()
    }

    // ---------------------------------------------------------------- listing

    async fn online_players(&self, inst: &Instance) -> Option<u32> {
        if inst.status() != Status::Running {
            return None;
        }
        let m = inst.meta();
        let out = tokio::time::timeout(
            Duration::from_secs(3),
            crate::rcon::exec(m.rcon_port, &m.rcon_password, "list"),
        )
        .await
        .ok()?
        .ok()?;
        let out = util::strip_colors(&out);
        let c = PLAYERS_RE.captures(&out)?;
        c.get(1).or(c.get(2))?.as_str().parse().ok()
    }

    pub async fn summary(&self, inst: &Arc<Instance>) -> InstanceSummary {
        let m = inst.meta();
        let (status, last_exit) = {
            let rt = inst.rt.lock().unwrap();
            (rt.status, rt.last_exit.clone())
        };
        InstanceSummary {
            name: m.name.clone(),
            status,
            core_jar: m.core_jar.clone(),
            game_port: m.game_port,
            memory: m.memory.clone(),
            autostart: m.autostart,
            java: m.java.clone(),
            online_players: self.online_players(inst).await,
            plugins: self.installed_plugins(&m.name),
            busy: inst.busy_info(),
            last_exit,
        }
    }

    pub async fn list(&self) -> Vec<InstanceSummary> {
        let mut out = Vec::new();
        for i in self.all() {
            out.push(self.summary(&i).await);
        }
        out
    }

    // ---------------------------------------------------------------- create

    pub async fn create(
        &self,
        agent: &str,
        name: &str,
        core_jar: &str,
        memory: &str,
        autostart: bool,
    ) -> Result<InstanceSummary> {
        validate_name("instance", name)?;
        if !self.cfg.eula_accepted {
            bail!(
                "the operator has not accepted the Minecraft EULA (https://aka.ms/MinecraftEULA) for this service. \
                 Ask the user to run setup.sh on the host (or set ABMPT_EULA_ACCEPTED=true after agreeing) and restart the container."
            );
        }
        let memory = normalize_memory(memory)?;
        if !self.jars.exists(core_jar) {
            bail!("core jar '{core_jar}' not found; see core_jar_list / core_jar_import");
        }
        let _g = self.registry_lock.lock().await;
        if self.instances.read().unwrap().contains_key(name) || self.dir(name).exists() {
            bail!("an instance named '{name}' already exists");
        }
        let used: Vec<u16> = self.all().iter().map(|i| i.meta().game_port).collect();
        let game_port = (self.cfg.game_port_min..=self.cfg.game_port_max)
            .find(|p| !used.contains(p))
            .with_context(|| {
                format!(
                    "no free game port: all {} ports in {}-{} are in use. Delete an instance first.",
                    self.cfg.game_port_max - self.cfg.game_port_min + 1,
                    self.cfg.game_port_min,
                    self.cfg.game_port_max
                )
            })?;
        let mut pw = [0u8; 24];
        rand::rng().fill_bytes(&mut pw);
        let meta = InstanceMeta {
            name: name.to_string(),
            core_jar: core_jar.to_string(),
            memory,
            autostart,
            game_port,
            rcon_port: self.cfg.rcon_port_for(game_port),
            rcon_password: hex::encode(pw),
            java: self.cfg.default_java.clone(),
            initialized: false,
            created_at: util::now_rfc3339(),
            created_by: agent.to_string(),
            eula_accepted_by: format!("operator (setup, {})", self.cfg.eula_accepted_at),
        };

        let dir = self.dir(name);
        let res: Result<()> = (|| {
            std::fs::create_dir_all(dir.join("plugins"))?;
            std::fs::write(
                dir.join("eula.txt"),
                format!(
                    "# EULA (https://aka.ms/MinecraftEULA) accepted by the service operator during setup ({}).\n# Instance created by agent '{agent}' at {}\neula=true\n",
                    self.cfg.eula_accepted_at, meta.created_at
                ),
            )?;
            let base = format!("#Minecraft server properties\nmotd={name}\n");
            std::fs::write(dir.join("server.properties"), self.managed_props(&base, &meta, false))?;
            self.save_meta(&meta)?;
            Ok(())
        })();
        if let Err(e) = res {
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::remove_file(self.meta_path(name));
            return Err(e);
        }
        self.redactor.add(&meta.rcon_password);
        let inst = Arc::new(Instance::new(meta));
        self.instances
            .write()
            .unwrap()
            .insert(name.to_string(), inst.clone());
        Ok(self.summary(&inst).await)
    }

    fn managed_props(&self, text: &str, m: &InstanceMeta, local_only: bool) -> String {
        props::set_many(
            text,
            &[
                ("server-port", m.game_port.to_string()),
                (
                    "server-ip",
                    if local_only { "127.0.0.1".into() } else { String::new() },
                ),
                ("enable-rcon", "true".into()),
                ("rcon.port", m.rcon_port.to_string()),
                ("rcon.password", m.rcon_password.clone()),
            ],
        )
    }

    fn apply_managed_props(&self, m: &InstanceMeta, local_only: bool) -> Result<()> {
        let p = self.dir(&m.name).join("server.properties");
        let cur = std::fs::read_to_string(&p).unwrap_or_default();
        let new = self.managed_props(&cur, m, local_only);
        if new != cur {
            util::write_atomic(&p, new.as_bytes())?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------- update / delete

    pub async fn update(
        &self,
        inst: &Arc<Instance>,
        memory: Option<&str>,
        autostart: Option<bool>,
        core_jar: Option<&str>,
    ) -> Result<InstanceSummary> {
        if (memory.is_some() || core_jar.is_some()) && inst.proc().is_some() {
            bail!(
                "instance '{}' is {:?}; stop it before changing memory or core jar",
                inst.name,
                inst.status()
            );
        }
        let mut m = inst.meta();
        if let Some(mem) = memory {
            m.memory = normalize_memory(mem)?;
        }
        if let Some(j) = core_jar {
            if !self.jars.exists(j) {
                bail!("core jar '{j}' not found");
            }
            m.core_jar = j.to_string();
        }
        if let Some(a) = autostart {
            m.autostart = a;
        }
        self.save_meta(&m)?;
        *inst.meta.lock().unwrap() = m;
        Ok(self.summary(inst).await)
    }

    pub async fn delete(&self, inst: &Arc<Instance>) -> Result<String> {
        let _g = self.registry_lock.lock().await;
        let stop = if inst.proc().is_some() {
            Some(self.stop_locked(inst).await)
        } else {
            None
        };
        let m = inst.meta();
        let dir = self.dir(&m.name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))?;
        }
        let b = self.paths.backups_dir(&m.name);
        if b.exists() {
            std::fs::remove_dir_all(&b)?;
        }
        std::fs::remove_file(self.meta_path(&m.name))?;
        self.instances.write().unwrap().remove(&m.name);
        self.redactor.remove(&m.rcon_password);
        let mut msg = format!(
            "Instance '{}' was PERMANENTLY DELETED: its world, plugins, configs, logs and backups are gone and cannot be recovered. Game port {} is free again.",
            m.name, m.game_port
        );
        if let Some(s) = stop {
            msg.push_str(&format!(
                " It was running and was shut down first{}.",
                if s.forced_kill { " (forced kill after timeout)" } else { "" }
            ));
        }
        Ok(msg)
    }

    // ---------------------------------------------------------------- start / stop

    /// Spawn the server and wait until it finishes booting. Caller holds the busy guard.
    async fn boot_locked(&self, inst: &Arc<Instance>, init: bool) -> Result<BootReport> {
        if let Some(p) = inst.proc()
            && !p.has_exited()
        {
            bail!("instance '{}' is already {:?}", inst.name, inst.status());
        }
        let booting = if init { Status::Initializing } else { Status::Starting };
        inst.set_status(booting);
        let queue = self.boot_queue.lock().await;

        let m = inst.meta();
        let result = (|| -> Result<Arc<ProcHandle>> {
            let java = self
                .cfg
                .javas
                .get(&m.java)
                .with_context(|| format!("java runtime '{}' is not available", m.java))?;
            let jar = self.jars.path(&m.core_jar);
            if !jar.exists() {
                bail!("core jar '{}' is missing", m.core_jar);
            }
            self.apply_managed_props(&m, init)?;
            let dir = self.dir(&m.name);
            let my_gen = {
                let mut rt = inst.rt.lock().unwrap();
                rt.generation += 1;
                rt.generation
            };
            let weak = Arc::downgrade(inst);
            let proc = ProcHandle::spawn(
                SpawnSpec {
                    java,
                    dir: &dir,
                    jar: &jar,
                    memory: &m.memory,
                },
                move |desc| {
                    if let Some(inst) = weak.upgrade() {
                        let idle = inst.idle_status();
                        let mut rt = inst.rt.lock().unwrap();
                        if rt.generation == my_gen {
                            if rt.status == Status::Running {
                                tracing::warn!("instance '{}' exited unexpectedly: {desc}", inst.name);
                            }
                            rt.proc = None;
                            rt.status = idle;
                            rt.last_exit = Some(desc);
                        }
                    }
                },
            )?;
            inst.rt.lock().unwrap().proc = Some(proc.clone());
            Ok(proc)
        })();
        let proc = match result {
            Ok(p) => p,
            Err(e) => {
                inst.set_status(inst.idle_status());
                return Err(e);
            }
        };

        let timeout = self.cfg.boot_timeout;
        let outcome = tokio::select! {
            _ = proc.wait_ready() => None,
            d = proc.wait_exit() => Some(format!("server process ended during boot ({d})")),
            _ = tokio::time::sleep(timeout) => Some(format!("server did not finish booting within {}s", timeout.as_secs())),
        };
        let mut failure = outcome;
        if failure.is_some() && !proc.has_exited() {
            inst.set_status(Status::Stopping);
            let (forced, _) = proc.stop(self.cfg.stop_timeout).await;
            if forced
                && let Some(f) = failure.as_mut()
            {
                f.push_str("; it was then force-killed");
            }
        }
        drop(queue);
        if failure.is_none() && !init {
            inst.set_status(Status::Running);
        }
        let (errors, error_count, warning_count) = proc.boot_summary(15);
        let ok = failure.is_none();
        Ok(BootReport {
            ok,
            status: inst.status(),
            seconds: (proc.started.elapsed().as_millis() as f64 / 1000.0),
            game_port: m.game_port,
            failure,
            error_count,
            warning_count,
            errors: errors.into_iter().map(|e| self.redactor.redact(&e)).collect(),
            console_tail: if ok {
                Vec::new()
            } else {
                proc.tail(40).into_iter().map(|l| self.redactor.redact(&l)).collect()
            },
        })
    }

    async fn stop_locked(&self, inst: &Arc<Instance>) -> StopReport {
        let Some(proc) = inst.proc() else {
            return StopReport {
                was_running: false,
                forced_kill: false,
                seconds: 0.0,
                exit: None,
            };
        };
        inst.set_status(Status::Stopping);
        let t = std::time::Instant::now();
        let (forced, desc) = proc.stop(self.cfg.stop_timeout).await;
        // on_exit normally resets this; make sure we never stay in Stopping.
        if inst.status() == Status::Stopping {
            inst.set_status(inst.idle_status());
        }
        StopReport {
            was_running: true,
            forced_kill: forced,
            seconds: t.elapsed().as_millis() as f64 / 1000.0,
            exit: Some(desc),
        }
    }

    pub async fn start(&self, inst: &Arc<Instance>) -> Result<BootReport> {
        self.boot_locked(inst, false).await
    }

    pub async fn stop(&self, inst: &Arc<Instance>) -> StopReport {
        self.stop_locked(inst).await
    }

    pub async fn restart(&self, inst: &Arc<Instance>) -> Result<(StopReport, BootReport)> {
        let s = self.stop_locked(inst).await;
        let b = self.boot_locked(inst, false).await?;
        Ok((s, b))
    }

    pub async fn initialize(&self, inst: &Arc<Instance>) -> Result<InitReport> {
        if inst.proc().is_some() {
            bail!("instance '{}' is {:?}; stop it before initializing", inst.name, inst.status());
        }
        let dir = self.dir(&inst.name);
        let before = files::config_snapshot(&dir);
        let boot = self.boot_locked(inst, true).await?;
        let shutdown = if boot.ok {
            let s = self.stop_locked(inst).await;
            let mut m = inst.meta();
            m.initialized = true;
            self.save_meta(&m)?;
            *inst.meta.lock().unwrap() = m;
            if inst.status() == Status::Created {
                inst.set_status(Status::Stopped);
            }
            Some(s)
        } else {
            None
        };
        // Back to the normal binding (players allowed) for the next real start.
        self.apply_managed_props(&inst.meta(), false)?;
        let after = files::config_snapshot(&dir);
        let mut new_files = Vec::new();
        let mut modified = Vec::new();
        for (k, v) in &after {
            match before.get(k) {
                None => new_files.push(k.clone()),
                Some(old) if old != v => modified.push(k.clone()),
                _ => {}
            }
        }
        Ok(InitReport {
            ok: boot.ok,
            status: inst.status(),
            boot,
            shutdown,
            new_config_files: new_files,
            modified_config_files: modified,
        })
    }

    // ---------------------------------------------------------------- console

    pub async fn console(&self, inst: &Arc<Instance>, command: &str) -> Result<String> {
        let st = inst.status();
        if st != Status::Running {
            bail!("instance '{}' is not running (status: {st:?}); start it first", inst.name);
        }
        let cmd = command.trim().trim_start_matches('/');
        if cmd.is_empty() {
            bail!("command is empty");
        }
        let first = cmd.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        if matches!(first.as_str(), "stop" | "restart" | "minecraft:stop") {
            bail!("use the instance_stop / instance_restart tools instead of the '{first}' console command");
        }
        let m = inst.meta();
        let out = crate::rcon::exec(m.rcon_port, &m.rcon_password, cmd)
            .await
            .map_err(|e| anyhow::anyhow!("console command failed: {e}"))?;
        let out = util::strip_colors(&out);
        Ok(if out.trim().is_empty() {
            "(no output)".to_string()
        } else {
            util::truncate_head(&out, 32 * 1024)
        })
    }

    // ---------------------------------------------------------------- autostart / shutdown

    pub async fn autostart_all(self: Arc<Self>) {
        for inst in self.all() {
            if !inst.meta().autostart {
                continue;
            }
            tokio::time::sleep(self.cfg.autostart_delay).await;
            let Ok(_g) = inst.acquire("system:autostart", "autostart") else {
                continue;
            };
            match self.boot_locked(&inst, false).await {
                Ok(r) if r.ok => tracing::info!("autostarted '{}' in {:.1}s", inst.name, r.seconds),
                Ok(r) => tracing::error!("autostart of '{}' failed: {:?}", inst.name, r.failure),
                Err(e) => tracing::error!("autostart of '{}' failed: {e:#}", inst.name),
            }
        }
    }

    /// Gracefully stop every running server (used on container shutdown).
    pub async fn shutdown_all(&self) {
        let procs: Vec<_> = self
            .all()
            .into_iter()
            .filter_map(|i| i.proc().map(|p| (i, p)))
            .collect();
        if procs.is_empty() {
            return;
        }
        tracing::info!("stopping {} running server(s)", procs.len());
        let grace = self.cfg.stop_timeout;
        futures::future::join_all(procs.into_iter().map(|(i, p)| async move {
            i.set_status(Status::Stopping);
            let (forced, d) = p.stop(grace).await;
            tracing::info!("'{}' stopped ({d}{})", i.name, if forced { ", forced" } else { "" });
        }))
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory() {
        assert_eq!(normalize_memory("2g").unwrap(), "2G");
        assert_eq!(normalize_memory("1536M").unwrap(), "1536M");
        assert_eq!(normalize_memory("2GB").unwrap(), "2G");
        assert!(normalize_memory("256M").is_err());
        assert!(normalize_memory("2T").is_err());
        assert!(normalize_memory("-Xmx2G").is_err());
    }
}
