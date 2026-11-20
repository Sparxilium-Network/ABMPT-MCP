//! HTTPS (or plain HTTP when ABMPT_TLS=off) server: `/mcp` (Streamable HTTP MCP), `PUT /upload`, `/healthz`.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Query, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, put},
};
use futures::StreamExt;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use crate::{
    audit::Audit,
    auth::{self, Agent, TokenStore},
    config::Config,
    instance::Manager,
    jarinfo,
    mcp::AbmptMcp,
    redact::Redactor,
    store::sanitize_jar_filename,
    tls,
};

#[derive(Clone)]
struct AppState {
    mgr: Arc<Manager>,
    audit: Arc<Audit>,
}

#[derive(Deserialize)]
struct UploadQuery {
    filename: String,
}

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({"error": msg.into()}))).into_response()
}

async fn upload(
    State(st): State<AppState>,
    Extension(Agent(agent)): Extension<Agent>,
    Query(q): Query<UploadQuery>,
    body: Body,
) -> Response {
    let filename = match sanitize_jar_filename(&q.filename) {
        Ok(f) => f,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    };
    let up = &st.mgr.uploads;
    let id = up.new_id();
    let tmp = up.temp_path(&id);
    let max = st.mgr.cfg.max_upload_bytes;
    let res: Result<(), (StatusCode, String)> = async {
        let mut f = tokio::fs::File::create(&tmp)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let mut stream = body.into_data_stream();
        let mut total: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| (StatusCode::BAD_REQUEST, format!("upload interrupted: {e}")))?;
            total += chunk.len() as u64;
            if total > max {
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!("upload exceeds {} MB", max / 1024 / 1024),
                ));
            }
            f.write_all(&chunk)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        f.flush().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if total == 0 {
            return Err((StatusCode::BAD_REQUEST, "empty upload".into()));
        }
        Ok(())
    }
    .await;
    let detail = json!({"filename": filename});
    if let Err((code, msg)) = res {
        let _ = tokio::fs::remove_file(&tmp).await;
        st.audit.record(&agent, None, "upload", detail, &Err(msg.clone()));
        return err(code, msg);
    }
    let meta = match up.finalize(&id, filename, &agent) {
        Ok(m) => m,
        Err(e) => {
            let msg = format!("{e:#}");
            st.audit.record(&agent, None, "upload", detail, &Err(msg.clone()));
            return err(StatusCode::BAD_REQUEST, msg);
        }
    };
    let plugin = jarinfo::plugin_info(&up.jar_path(&id)).ok();
    st.audit.record(
        &agent,
        None,
        "upload",
        json!({"filename": meta.filename, "size": meta.size, "sha256": meta.sha256, "upload_id": meta.upload_id}),
        &Ok("stored".into()),
    );
    let hint = if plugin.is_some() {
        "Looks like a plugin: use plugin_deploy with this upload_id."
    } else {
        "No plugin.yml: if this is a Paper/Folia server jar use core_jar_import with this upload_id."
    };
    (
        StatusCode::CREATED,
        Json(json!({
            "upload_id": meta.upload_id,
            "filename": meta.filename,
            "size": meta.size,
            "sha256": meta.sha256,
            "plugin": plugin,
            "expires_in_hours": 24,
            "next": hint,
        })),
    )
        .into_response()
}

pub async fn serve(cfg: Config) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let paths = cfg.paths();
    paths.ensure()?;
    let tls_files = if cfg.tls_enabled {
        Some(tls::ensure(&paths.tls_dir(), &cfg.tls_sans)?)
    } else {
        None
    };
    let redactor = Redactor::default();
    let audit = Arc::new(Audit::new(paths.audit_file(), redactor.clone()));
    let tokens = Arc::new(TokenStore::new(paths.tokens_file()));
    if tokens.list().map(|t| t.is_empty()).unwrap_or(true) {
        tracing::warn!("no tokens exist yet: create one with `abmpt token create <agent-name>`");
    }
    let mgr = Manager::new(cfg.clone(), redactor)?;
    mgr.uploads.cleanup();

    let mcp_cfg = StreamableHttpServerConfig::default()
        .with_allowed_hosts(cfg.tls_sans.clone())
        .with_sse_keep_alive(Some(Duration::from_secs(15)));
    let (m2, a2) = (mgr.clone(), audit.clone());
    let mcp = StreamableHttpService::new(
        move || Ok(AbmptMcp::new(m2.clone(), a2.clone())),
        Arc::new(LocalSessionManager::default()),
        mcp_cfg,
    );

    let state = AppState {
        mgr: mgr.clone(),
        audit: audit.clone(),
    };
    let app = Router::new()
        .nest_service("/mcp", mcp)
        .route(
            "/upload",
            put(upload).layer(DefaultBodyLimit::disable()).with_state(state),
        )
        .layer(middleware::from_fn_with_state(tokens, auth::require_token))
        .route("/healthz", get(|| async { "ok" }));

    let rustls_cfg = match &tls_files {
        Some(f) => Some(
            axum_server::tls_rustls::RustlsConfig::from_pem_file(&f.cert, &f.key)
                .await
                .context("load TLS certificate")?,
        ),
        None => None,
    };
    let handle = axum_server::Handle::new();
    match &tls_files {
        Some(f) => tracing::info!(
            "listening on https://{} (TLS SANs: {:?}); CA certificate: {}",
            cfg.listen,
            cfg.tls_sans,
            f.ca_cert.display()
        ),
        None => {
            tracing::warn!(
                "TLS is OFF (ABMPT_TLS=off): tokens, uploads and config file contents travel unencrypted. \
                 Use only on a trusted development network."
            );
            tracing::info!("listening on http://{} (accepted hosts: {:?})", cfg.listen, cfg.tls_sans);
        }
    }

    // Periodic upload cleanup.
    let m3 = mgr.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            m3.uploads.cleanup();
        }
    });
    tokio::spawn(mgr.clone().autostart_all());

    let h2 = handle.clone();
    let m4 = mgr.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("shutdown requested: stopping servers first");
        m4.shutdown_all().await;
        h2.graceful_shutdown(Some(Duration::from_secs(5)));
    });

    match rustls_cfg {
        Some(rc) => axum_server::bind_rustls(cfg.listen, rc)
            .handle(handle)
            .serve(app.into_make_service())
            .await
            .context("HTTPS server failed")?,
        None => axum_server::bind(cfg.listen)
            .handle(handle)
            .serve(app.into_make_service())
            .await
            .context("HTTP server failed")?,
    }
    // In case the server ended for another reason, never leave servers running unsaved.
    mgr.shutdown_all().await;
    tracing::info!("bye");
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
        tokio::select! {
            _ = term.recv() => {},
            _ = int.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
