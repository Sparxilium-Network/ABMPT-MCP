//! MCP tool surface.

use std::sync::Arc;

use http::request::Parts;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::Extension, wrapper::Parameters},
    model::{ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    audit::Audit,
    auth::Agent,
    instance::{Instance, Manager},
    logs::Level,
};

#[derive(Clone)]
pub struct AbmptMcp {
    mgr: Arc<Manager>,
    audit: Arc<Audit>,
    #[allow(dead_code, reason = "used by the tool_handler macro")]
    tool_router: ToolRouter<Self>,
}

fn js<T: Serialize>(v: &T) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(v)?)
}

fn agent_of(parts: &Parts) -> String {
    parts
        .extensions
        .get::<Agent>()
        .map(|a| a.0.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

// ------------------------------------------------------------------ parameter types

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NameArg {
    /// Instance name.
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CoreJarImportArg {
    /// `upload_id` returned by `PUT /upload`.
    pub upload_id: String,
    /// Name to store the core jar under, e.g. `paper-1.21.10-120`. a-z, 0-9, '_' or '-'.
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CoreJarNameArg {
    /// Core jar name.
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateArg {
    /// New instance name: 1-32 chars of a-z, 0-9, '_' or '-'.
    pub name: String,
    /// Name of a stored core jar (see core_jar_list).
    pub core_jar: String,
    /// Heap size, e.g. "2G" or "1536M" (min 512M).
    pub memory: String,
    /// Start this instance automatically when the MCP service starts. Default false.
    pub autostart: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateArg {
    pub name: String,
    /// New heap size (instance must be stopped).
    pub memory: Option<String>,
    pub autostart: Option<bool>,
    /// New core jar name (instance must be stopped).
    pub core_jar: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteArg {
    pub name: String,
    /// Must be true. Deletion is permanent and cannot be undone.
    pub confirm: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConsoleArg {
    pub name: String,
    /// Console command without leading slash, e.g. "list" or "lp user Steve info".
    pub command: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LevelArg {
    All,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LogsArg {
    pub name: String,
    /// Number of log entries (default 100, max 1000). Stack traces count as one entry.
    pub lines: Option<usize>,
    /// Minimum level: all | info | warn | error. Default all.
    pub level: Option<LevelArg>,
    /// Only entries tagged with this plugin, e.g. "LuckPerms".
    pub plugin: Option<String>,
    /// Only entries containing this text (case-insensitive).
    pub contains: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeployArg {
    pub name: String,
    /// `upload_id` returned by `PUT /upload`.
    pub upload_id: String,
    /// Restart the instance if it is running, then report whether the plugin loaded. Default false.
    pub restart: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PluginArg {
    pub name: String,
    /// Plugin name as in plugin.yml (e.g. "LuckPerms"), not the file name.
    pub plugin: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RollbackArg {
    pub name: String,
    /// Plugin name as in plugin.yml.
    pub plugin: String,
    /// Restart the instance if running. Default false.
    pub restart: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileListArg {
    pub name: String,
    /// Folder relative to the instance root. Default: root.
    pub path: Option<String>,
    /// How many folder levels to expand (1-5, default 2).
    pub depth: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileArg {
    pub name: String,
    /// File path relative to the instance root, e.g. "plugins/LuckPerms/config.yml".
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileWriteArg {
    pub name: String,
    pub path: String,
    /// Full new file content. Lines still containing __ABMPT_REDACTED__ keep their stored value.
    pub content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileEditArg {
    pub name: String,
    pub path: String,
    /// Exact text to replace, as shown by file_read. Must match exactly once.
    pub old_string: String,
    pub new_string: String,
}

// ------------------------------------------------------------------ helpers

impl AbmptMcp {
    pub fn new(mgr: Arc<Manager>, audit: Arc<Audit>) -> Self {
        Self {
            mgr,
            audit,
            tool_router: Self::tool_router(),
        }
    }

    /// Run an operation: redact the output, record it in the audit log.
    async fn run(
        &self,
        parts: &Parts,
        action: &str,
        instance: Option<&str>,
        detail: Value,
        f: impl AsyncFnOnce(String) -> anyhow::Result<String>,
    ) -> Result<String, String> {
        let agent = agent_of(parts);
        let r = f(agent.clone())
            .await
            .map(|s| self.mgr.redactor.redact(&s))
            .map_err(|e| self.mgr.redactor.redact(&format!("{e:#}")));
        self.audit.record(&agent, instance, action, detail, &r);
        r
    }

    /// Like `run`, but holds the instance's exclusive lock for state-changing operations.
    async fn run_locked(
        &self,
        parts: &Parts,
        action: &str,
        name: &str,
        detail: Value,
        f: impl AsyncFnOnce(Arc<Instance>) -> anyhow::Result<String>,
    ) -> Result<String, String> {
        self.run(parts, action, Some(name), detail, async |agent| {
            let inst = self.mgr.get(name)?;
            let _guard = inst.acquire(&agent, action)?;
            f(inst).await
        })
        .await
    }
}

// ------------------------------------------------------------------ tools

#[tool_router]
impl AbmptMcp {
    #[tool(description = "Service overview: game port range, how to upload jars, available Java runtimes, limits. Call this first if unsure how to use the service.")]
    async fn server_info(&self, Extension(parts): Extension<Parts>) -> Result<String, String> {
        let c = &self.mgr.cfg;
        self.run(&parts, "server_info", None, json!({}), async |_| {
            js(&json!({
                "game_ports": format!("{}-{}", c.game_port_min, c.game_port_max),
                "java_runtimes": c.javas.keys().collect::<Vec<_>>(),
                "default_java": c.default_java,
                "boot_timeout_secs": c.boot_timeout.as_secs(),
                "stop_timeout_secs": c.stop_timeout.as_secs(),
                "max_upload_mb": c.max_upload_bytes / 1024 / 1024,
                "upload": upload_help(c.tls_enabled),
                "workflow": WORKFLOW_HELP,
            }))
        })
        .await
    }

    #[tool(description = "List stored server core jars (Paper/Folia) usable for instance_create.")]
    async fn core_jar_list(&self, Extension(parts): Extension<Parts>) -> Result<String, String> {
        self.run(&parts, "core_jar_list", None, json!({}), async |_| js(&self.mgr.jars.list()))
            .await
    }

    #[tool(description = "Store an uploaded Paper/Folia jar as a named server core jar. Upload the file first with PUT /upload (see server_info).")]
    async fn core_jar_import(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<CoreJarImportArg>,
    ) -> Result<String, String> {
        self.run(
            &parts,
            "core_jar_import",
            None,
            json!({"upload_id": a.upload_id, "name": a.name}),
            async |agent| {
                let (up, path) = self.mgr.uploads.get(&a.upload_id)?;
                js(&self.mgr.jars.import(&a.name, &path, &up, &agent)?)
            },
        )
        .await
    }

    #[tool(description = "Delete a stored core jar that no instance uses.")]
    async fn core_jar_delete(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<CoreJarNameArg>,
    ) -> Result<String, String> {
        self.run(&parts, "core_jar_delete", None, json!({"name": a.name}), async |_| {
            let users: Vec<String> = self
                .mgr
                .all()
                .iter()
                .filter(|i| i.meta().core_jar == a.name)
                .map(|i| i.name.clone())
                .collect();
            if !users.is_empty() {
                anyhow::bail!("core jar '{}' is used by: {}", a.name, users.join(", "));
            }
            self.mgr.jars.delete(&a.name)?;
            Ok(format!("deleted core jar '{}'", a.name))
        })
        .await
    }

    #[tool(description = "List all instances with status (created|stopped|initializing|starting|running|stopping), core jar, game port, memory, autostart, online players and installed plugins.")]
    async fn instance_list(&self, Extension(parts): Extension<Parts>) -> Result<String, String> {
        self.run(&parts, "instance_list", None, json!({}), async |_| js(&self.mgr.list().await))
            .await
    }

    #[tool(description = "Show one instance (same fields as instance_list).")]
    async fn instance_get(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<NameArg>,
    ) -> Result<String, String> {
        self.run(&parts, "instance_get", Some(&a.name), json!({}), async |_| {
            let i = self.mgr.get(&a.name)?;
            js(&self.mgr.summary(&i).await)
        })
        .await
    }

    #[tool(description = "Create a new instance. It is NOT started and plugins/ is empty. A free game port is assigned automatically. server.properties is created so it can be edited before first boot. The Minecraft EULA is accepted automatically because the operator agreed to it when installing the service.")]
    async fn instance_create(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<CreateArg>,
    ) -> Result<String, String> {
        self.run(
            &parts,
            "instance_create",
            Some(&a.name),
            json!({"core_jar": a.core_jar, "memory": a.memory, "autostart": a.autostart}),
            async |agent| {
                let s = self
                    .mgr
                    .create(&agent, &a.name, &a.core_jar, &a.memory, a.autostart.unwrap_or(false))
                    .await?;
                js(&s)
            },
        )
        .await
    }

    #[tool(description = "Initialize an instance: boot it once (bound to 127.0.0.1 so no player can join) so the server and all plugins generate their config files, then shut it down cleanly. Reports generated files and boot errors. Can be repeated after deploying new plugins. Waits until done (first boot of a Paper jar can take minutes).")]
    async fn instance_initialize(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<NameArg>,
    ) -> Result<String, String> {
        self.run_locked(&parts, "instance_initialize", &a.name, json!({}), async |inst| {
            js(&self.mgr.initialize(&inst).await?)
        })
        .await
    }

    #[tool(description = "Change memory, autostart or core jar. Memory and core jar require the instance to be stopped; changes apply on next start.")]
    async fn instance_update(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<UpdateArg>,
    ) -> Result<String, String> {
        self.run_locked(
            &parts,
            "instance_update",
            &a.name,
            json!({"memory": a.memory, "autostart": a.autostart, "core_jar": a.core_jar}),
            async |inst| {
                js(&self
                    .mgr
                    .update(&inst, a.memory.as_deref(), a.autostart, a.core_jar.as_deref())
                    .await?)
            },
        )
        .await
    }

    #[tool(description = "PERMANENTLY delete an instance and all its data (world, plugins, configs, logs, backups). A running instance is shut down first. Cannot be undone. Requires confirm=true.")]
    async fn instance_delete(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<DeleteArg>,
    ) -> Result<String, String> {
        self.run_locked(&parts, "instance_delete", &a.name, json!({"confirm": a.confirm}), async |inst| {
            if !a.confirm {
                anyhow::bail!("deletion is permanent; call again with confirm=true if the user really wants this");
            }
            self.mgr.delete(&inst).await
        })
        .await
    }

    #[tool(description = "Start an instance and wait until it has fully booted. Returns success/failure and errors seen during boot. Boots are queued: only one instance boots at a time.")]
    async fn instance_start(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<NameArg>,
    ) -> Result<String, String> {
        self.run_locked(&parts, "instance_start", &a.name, json!({}), async |inst| {
            js(&self.mgr.start(&inst).await?)
        })
        .await
    }

    #[tool(description = "Stop an instance gracefully (saves worlds). Force-kills only if it does not stop in time.")]
    async fn instance_stop(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<NameArg>,
    ) -> Result<String, String> {
        self.run_locked(&parts, "instance_stop", &a.name, json!({}), async |inst| {
            js(&self.mgr.stop(&inst).await)
        })
        .await
    }

    #[tool(description = "Restart an instance (graceful stop, then start) and wait until booted. Returns the boot result.")]
    async fn instance_restart(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<NameArg>,
    ) -> Result<String, String> {
        self.run_locked(&parts, "instance_restart", &a.name, json!({}), async |inst| {
            let (s, b) = self.mgr.restart(&inst).await?;
            js(&json!({"stop": s, "boot": b}))
        })
        .await
    }

    #[tool(description = "Run a server console command on a running instance and return its output (color codes removed). RCON credentials are handled internally.")]
    async fn console_command(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<ConsoleArg>,
    ) -> Result<String, String> {
        self.run(
            &parts,
            "console_command",
            Some(&a.name),
            json!({"command": a.command}),
            async |_| {
                let inst = self.mgr.get(&a.name)?;
                self.mgr.console(&inst, &a.command).await
            },
        )
        .await
    }

    #[tool(description = "Read the latest log entries of an instance (logs/latest.log). Filter by minimum level (e.g. error), by plugin tag, or by text. Output is size-limited.")]
    async fn logs_tail(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<LogsArg>,
    ) -> Result<String, String> {
        self.run(
            &parts,
            "logs_tail",
            Some(&a.name),
            json!({"lines": a.lines, "plugin": a.plugin, "contains": a.contains}),
            async |_| {
                let inst = self.mgr.get(&a.name)?;
                let level = match a.level {
                    None | Some(LevelArg::All) => Level::Debug,
                    Some(LevelArg::Info) => Level::Info,
                    Some(LevelArg::Warn) => Level::Warn,
                    Some(LevelArg::Error) => Level::Error,
                };
                self.mgr.logs_tail(
                    &inst,
                    a.lines.unwrap_or(100),
                    level,
                    a.plugin.as_deref(),
                    a.contains.as_deref(),
                )
            },
        )
        .await
    }

    #[tool(description = "List installed plugins of an instance and the stored backups of previous versions.")]
    async fn plugin_list(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<NameArg>,
    ) -> Result<String, String> {
        self.run(&parts, "plugin_list", Some(&a.name), json!({}), async |_| {
            let inst = self.mgr.get(&a.name)?;
            let plugins = self.mgr.installed_plugins(&inst.name);
            let backups: Vec<Value> = plugins
                .iter()
                .map(|p| json!({"plugin": p.name, "backups": self.mgr.plugin_backups(&inst.name, &p.name)}))
                .collect();
            js(&json!({"installed": plugins, "backups": backups}))
        })
        .await
    }

    #[tool(description = "Deploy an uploaded plugin jar into plugins/, replacing any installed jar of the same plugin (by plugin.yml name). The replaced jar is kept as a backup for plugin_rollback. With restart=true a running instance is restarted and the result tells whether the plugin loaded.")]
    async fn plugin_deploy(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<DeployArg>,
    ) -> Result<String, String> {
        self.run_locked(
            &parts,
            "plugin_deploy",
            &a.name,
            json!({"upload_id": a.upload_id, "restart": a.restart}),
            async |inst| {
                js(&self
                    .mgr
                    .plugin_deploy(&inst, &a.upload_id, a.restart.unwrap_or(false))
                    .await?)
            },
        )
        .await
    }

    #[tool(description = "Remove a plugin jar from plugins/ (kept as a backup; its config folder stays). Restart to unload it.")]
    async fn plugin_remove(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<PluginArg>,
    ) -> Result<String, String> {
        self.run_locked(&parts, "plugin_remove", &a.name, json!({"plugin": a.plugin}), async |inst| {
            self.mgr.plugin_remove(&inst, &a.plugin)
        })
        .await
    }

    #[tool(description = "Restore the most recent backup of a plugin (the previous version). The current version becomes a backup, so rolling back twice re-applies it.")]
    async fn plugin_rollback(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<RollbackArg>,
    ) -> Result<String, String> {
        self.run_locked(
            &parts,
            "plugin_rollback",
            &a.name,
            json!({"plugin": a.plugin, "restart": a.restart}),
            async |inst| {
                js(&self
                    .mgr
                    .plugin_rollback(&inst, &a.plugin, a.restart.unwrap_or(false))
                    .await?)
            },
        )
        .await
    }

    #[tool(description = "List files and folders inside an instance folder. World, log and cache folders are not expanded.")]
    async fn file_list(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<FileListArg>,
    ) -> Result<String, String> {
        self.run(&parts, "file_list", Some(&a.name), json!({"path": a.path}), async |_| {
            let inst = self.mgr.get(&a.name)?;
            js(&self
                .mgr
                .files_list(&inst, a.path.as_deref().unwrap_or(""), a.depth.unwrap_or(2))?)
        })
        .await
    }

    #[tool(description = "Read a text config file inside an instance folder (server.properties, Paper configs, plugins/*/config.yml, ...). Passwords, tokens and similar values are shown as __ABMPT_REDACTED__.")]
    async fn file_read(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<FileArg>,
    ) -> Result<String, String> {
        self.run(&parts, "file_read", Some(&a.name), json!({"path": a.path}), async |_| {
            let inst = self.mgr.get(&a.name)?;
            self.mgr.file_read(&inst, &a.path)
        })
        .await
    }

    #[tool(description = "Write a whole text config file inside an instance folder. The previous version is backed up automatically. Lines left exactly as read with __ABMPT_REDACTED__ keep their stored secret; to set a secret, write its real value. MCP-managed keys (server-port, server-ip, enable-rcon, rcon.port, rcon.password) cannot be changed.")]
    async fn file_write(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<FileWriteArg>,
    ) -> Result<String, String> {
        // Content is not logged: it may contain secrets supplied by the user.
        self.run_locked(
            &parts,
            "file_write",
            &a.name,
            json!({"path": a.path, "bytes": a.content.len()}),
            async |inst| self.mgr.file_write(&inst, &a.path, &a.content),
        )
        .await
    }

    #[tool(description = "Replace one exact snippet in a text config file (as shown by file_read). Safer than file_write for small changes. Same backup and secret rules as file_write.")]
    async fn file_edit(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<FileEditArg>,
    ) -> Result<String, String> {
        self.run_locked(&parts, "file_edit", &a.name, json!({"path": a.path}), async |inst| {
            self.mgr.file_edit(&inst, &a.path, &a.old_string, &a.new_string)
        })
        .await
    }
}

pub fn upload_help(tls: bool) -> String {
    let (cacert, scheme) = if tls { ("--cacert <ca.crt> ", "https") } else { ("", "http") };
    format!(
        "Jars are uploaded with an HTTP PUT, not through tool arguments: \
curl --fail {cacert}-H \"Authorization: Bearer $ABMPT_TOKEN\" --upload-file <file.jar> \
\"{scheme}://<host>:3307/upload?filename=<file.jar>\" . The JSON response contains upload_id \
(valid 24h) for core_jar_import or plugin_deploy. Keep the token in the ABMPT_TOKEN environment \
variable; never print it."
    )
}

pub const WORKFLOW_HELP: &str = "1) upload Paper jar + core_jar_import  2) instance_create  \
3) upload plugins + plugin_deploy  4) instance_initialize  5) file_read / file_edit configs  \
6) instance_start  7) console_command / logs_tail / plugin_deploy(restart=true) / plugin_rollback  \
8) instance_delete when the user asks.";

#[tool_handler]
impl ServerHandler for AbmptMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            format!(
                "Manages Paper/Folia Minecraft test servers. Every operation goes through these tools; \
                 RCON passwords are handled internally and never needed. State-changing operations on one \
                 instance are exclusive: if another agent is busy with it you get an error naming them. \
                 {} Typical workflow: {WORKFLOW_HELP}",
                upload_help(self.mgr.cfg.tls_enabled)
            ),
        )
    }
}
