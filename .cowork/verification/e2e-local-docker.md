# 本機 Docker 端到端驗收

日期：2026-11-19。環境：Windows 11 + Docker Desktop 29.9.3，映像 `abmpt:latest`，測試容器 `abmpt-test`（named volume，publish 3307 與 65000-65005）。用戶端：stdlib Python Streamable HTTP 腳本（strict TLS 驗證、以 CA 檔驗證）。核心 jar：使用者提供的 `folia-26.3-26.jar`。插件：Modrinth 的 LuckPerms-Bukkit 5.5.53 與 5.5.71（sha512 已比對）。測試結束後已刪除容器、volume 與 token 檔。

## 結果（對應需求書驗收方式）
| # | 項目 | 結果 |
|---|---|---|
| 1 | 上傳 jar → core_jar_import → instance_create 2G | 通過：status=created、plugins=[]、遊戲埠 65000 |
| 2 | 部署 LuckPerms + instance_initialize | 通過：約 20s，產生 plugins/LuckPerms/config.yml 等，自動關機 status=stopped |
| 3 | 改 LuckPerms DB 設定、遮蔽、備份 | 部分通過：密碼寫入磁碟、讀回顯示 `__ABMPT_REDACTED__`、修改無關欄位時密碼保留、每次寫入都有備份。**未連接真實資料庫**（本機無 DB，storage-method 改回 h2 後才開機） |
| 4 | 開機、玩家連線 | 部分通過：開機 9s；Server List Ping 從主機 127.0.0.1:65000 成功（Folia 26.3）。**未從區網另一台機器測試**，也未測 LuckPerms 連上 DB |
| 5 | `list` 無色碼 | 通過 |
| 6 | 最近的 ERROR log | 通過（當時沒有 error；level 與插件篩選正常） |
| 7 | 部署新版、重啟、確認載入、退回 | 通過：load_check.loaded=true（5.5.71），rollback 後 5.5.53 載入 |
| 8 | 兩個 token 同時重啟 | 通過：一個執行，另一個收到 busy 訊息並註明 agent |
| 9 | 改 RCON 密碼 | 通過：被拒絕；server-port、`../` 路徑、寫入 .jar 也被拒絕 |
| 10 | 自動啟動 | 以 `docker restart` 模擬：關機時先讓伺服器正常停止（exit 0、Folia 有存檔流程），重啟後自動上線。**未在 .206 實際重開主機** |
| 11 | 刪除 | 通過：confirm=false 被拒；刪除後 instances/meta/backups 都已清空 |

## 其他確認
- 沒有 token／錯誤 token 時回 401；revoke 後立即回 401。
- 在容器內以 grep 比對：工具輸出與 audit.jsonl 中都沒有出現 RCON 密碼與 DB 密碼（0 筆）。
- 發現並修正：rcgen 預設不帶 AKI，strict TLS 用戶端（Python 3.13+）驗證失敗 → 伺服器憑證加入 AKI，並以 `CERT_PROFILE` 觸發既有安裝重新簽發。
- 發現並修正：audit 的 result 摘要只記到 JSON 的第一行 `{` → 改為記錄壓縮後的 JSON（最多 300 bytes）。
- `cargo test` 19 項通過，`cargo clippy --all-targets` 沒有警告。

## 尚未驗證
- 在 .206 上部署、從區網另一台機器以 Claude Code／Codex／Gemini 實際連線（各用戶端信任 CA 的設定方式）。
- LuckPerms 連接真實 MariaDB／MySQL。
- 主機實際重開機。
