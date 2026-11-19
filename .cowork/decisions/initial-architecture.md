# 初始架構決策

日期：2026-11-19。依據 `mcp-server-requirements.md`，以下為使用者確認過的決策（需求書「已決策事項」之外的補充）。

| 項目 | 決定 | 理由 / 備註 |
|---|---|---|
| 實作語言 | Rust | 使用者指定；MCP SDK 預計用官方 `rmcp`（Streamable HTTP） |
| jar 上傳 | 同埠（3307）HTTP 上傳端點，例：`PUT /upload`，用同一組 token 驗證 | AI 無法把數十 MB 的 jar 以 base64 放入工具參數。AI 用 curl 上傳取得 upload id，再呼叫 MCP 工具使用 |
| 傳輸安全 | 內建 TLS | 純 HTTP 在區網可被竊聽 token |
| 憑證 | 首次啟動自動產生本地 CA + 伺服器憑證，存於掛載目錄；匯出 `ca.crt` 讓用戶端信任 | 文件需寫明各用戶端（Claude Code / Codex / Gemini）信任 CA 的步驟。不採「關閉驗證」 |
| token 管理 | 容器內 CLI：`docker exec <container> abmpt token create <name>` / `revoke` / `list` | 明文只顯示一次；只存 SHA-256 雜湊。不提供遠端發行端點 |
| 容器網路 | bridge；只 publish 3307/tcp 與 65000-65100/tcp | Paper RCON 無獨立綁定位址，host 網路會暴露 RCON。UDP 不開 |
| 初始化防連線 | 初始化期間暫時把 `server-ip` 設為 `127.0.0.1`，結束後還原 | 外部完全連不到；不留 whitelist 副作用 |
| 版本控制 | 已 `git init -b main`；`.claude/` `.codex/` `.gemini/` `/data/` `/target/` 加入 `.gitignore` | |
| 測試環境 | 本機 Windows 的 Docker Desktop 建置與測試；不碰 .206 | 2026-11-19 時 Docker Desktop daemon 未啟動，容器測試前需使用者開啟 |

## EULA（2026-11-19 使用者變更需求）
- 需求書原本寫「建立時由呼叫者明確同意 EULA」，使用者改為：**在安裝時由 `setup.sh` 告知「本程式會自動接受 EULA」並取得同意**（輸入 `yes`），寫入 `.env` 的 `ABMPT_EULA_ACCEPTED=true` 與同意時間／帳號。
- 服務端：`ABMPT_EULA_ACCEPTED` 不是 true 時，`instance_create` 會被拒絕；為 true 時自動寫入 `eula=true`，並在 eula.txt 註記是操作者在安裝時同意的。`instance_create` 已移除 `accept_eula` 參數。
- 不得在未經使用者同意的情況下設定 `ABMPT_EULA_ACCEPTED=true`。

## 實作時採用的預設（已實作）
- 敏感欄位遮蔽：依鍵名比對（password、passwd、secret、token、key 等，不分大小寫）。
- 遊戲埠在容器內與主機相同（65000-65100 一對一映射），RCON 用容器內另一段埠（不 publish）。
- 容器內以非 root 使用者執行。
- 需求書提到的 `sudo-requirements.md` 不在 repo 中，且屬範圍外。
