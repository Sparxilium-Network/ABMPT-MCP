# 開發說明

> 最後修改：2026-11-20 ｜ 基於 commit：`53199fc`

相關文件：[部署與維運](DEPLOY.md)、[設定 AI 用戶端](CLIENT-SETUP.md)。需求來源：[`mcp-server-requirements.md`](../mcp-server-requirements.md)。

## 架構

單一 Docker 容器，內含一個 Rust 執行檔 `abmpt`：

```
AI 用戶端 ──HTTPS:3307──► axum   （ABMPT_TLS=off 時為純 HTTP）
                           ├─ /healthz           （不需驗證）
                           ├─ /mcp    Streamable HTTP MCP（rmcp）  ┐ Bearer token
                           └─ PUT /upload  jar 上傳                  ┘ 中介層驗證
                                    │
                               Manager ──► java 子程序 × N（Paper／Folia）
                                    │            ▲ stdin "stop"、stdout 解析
                                    └── RCON（127.0.0.1，容器內）
```

- 伺服器是 `abmpt` 的子程序。服務會從 stdout 偵測 `Done (…)! For help` 判斷開機完成，並以 stdin `stop` 正常關機，逾時才 kill。
- 主控台指令走 RCON。RCON 埠與密碼只存在 `meta/<name>.json`，所有輸出都會經過 `Redactor` 清除。
- `ABMPT_TLS=off` 時不呼叫 `tls::ensure`，改用 `axum_server::bind`；`ABMPT_TLS_SANS` 在兩種模式下都是 MCP 端點允許的 Host 名稱。給 AI 的上傳說明（`mcp::upload_help`）會依模式切換 `https`／`http` 與 `--cacert`。
- 啟動時，服務會強制寫入 `server.properties` 中受管理的欄位（`server-port`、`server-ip`、`enable-rcon`、`rcon.port`、`rcon.password`）。初始化時把 `server-ip` 設為 `127.0.0.1`，讓玩家連不進來。

## 目錄

| 路徑 | 內容 |
|---|---|
| `src/` | 服務程式碼（見下表） |
| `Dockerfile` | 多階段建置：先用 Rust 編譯，再以 Temurin 25 JRE 作為執行映像，使用者 uid 10001 |
| `deploy/docker-compose.yml` | 部署用 compose，build context 為 `..` |
| `deploy/setup.sh` | 互動式安裝與升級（`setup`／`upgrade` 模式），寫入 `deploy/.env` |
| `deploy/.env.example` | `.env` 範本，帶 `ABMPT_CONFIG_VERSION` |

`.env` 新增或改名設定時，要提高 `setup.sh` 的 `CONFIG_VERSION` 與 `.env.example` 的 `ABMPT_CONFIG_VERSION`，並在 `setup.sh` 的升級迴圈加入就地修改舊檔案的步驟。

## 模組

| 檔案 | 職責 |
|---|---|
| `src/main.rs` | CLI：`serve`、`token create/revoke/list`、`ca` |
| `src/config.rs` | 環境變數設定與資料目錄配置 |
| `src/server.rs` | axum 路由、TLS 或純 HTTP、上傳端點、優雅關機 |
| `src/mcp.rs` | MCP 工具定義，以及稽核與鎖的包裝 |
| `src/auth.rs` | token 雜湊儲存、驗證中介層 |
| `src/audit.rs` | JSONL 稽核紀錄 |
| `src/tls.rs` | 本地 CA 與伺服器憑證產生 |
| `src/instance/mod.rs` | 實例註冊、建立／刪除、開機排隊、開關機、初始化、主控台 |
| `src/instance/process.rs` | Java 子程序監控（輸出、就緒、結束、kill） |
| `src/instance/files.rs` | 列出、讀寫設定檔，log 查詢 |
| `src/instance/plugins.rs` | 插件部署、移除、退回 |
| `src/store.rs` | 上傳暫存區、伺服器核心 jar 庫 |
| `src/mask.rs` | 敏感值遮蔽與寫回時還原 |
| `src/fsguard.rs` | 路徑限制在實例資料夾內 |
| `src/rcon.rs` | RCON 用戶端 |
| `src/logs.rs` | log 分段與篩選 |
| `src/props.rs` | `server.properties` 編輯 |
| `src/redact.rs` | 從輸出中清除已知機密 |
| `src/jarinfo.rs` | 讀取 `plugin.yml`／`paper-plugin.yml` |

## 並行規則

- 同一個實例上會改變狀態的操作（初始化、開關機、重啟、部署、退回、移除插件、寫檔、修改設定、刪除），都要先取得 `Instance::acquire` 鎖。後到的請求會直接收到錯誤，內容說明是誰在進行什麼操作，不會排隊等待。
- 不同實例的開機由 `boot_queue` 依序執行，後到的請求會等待。
- 建立與刪除實例由 `registry_lock` 序列化，避免名稱或遊戲埠衝突。

## 敏感值遮蔽

讀取設定檔時，鍵名符合 password／secret／token／api-key 等字樣的值，以及 URL 中的 `user:pass@`，都會顯示為 `__ABMPT_REDACTED__`。寫入時，仍含佔位字串的行必須與讀出時完全相同，服務才會換回原值；要設定新的密碼，就直接寫入真實值。

## 開發

```sh
cargo test
cargo clippy --all-targets
docker compose build
```

本機開發可以直接執行 `abmpt serve`，需要設定 `ABMPT_DATA_DIR`，並以 `ABMPT_JAVA_25` 指向 java 執行檔。

新增其他 Java 版本時，在映像檔中安裝到另一個路徑，並加上 `ABMPT_JAVA_<版本>` 環境變數。實例 metadata 的 `java` 欄位已預留，目前固定使用預設版本。
