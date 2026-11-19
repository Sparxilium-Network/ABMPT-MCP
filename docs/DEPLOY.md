# 部署與維運

> 最後修改：2026-11-19 ｜ 基於 commit：（初始版本）

## 前置需求

- Linux 主機，已安裝 Docker Engine 與 Compose plugin。
- Docker 服務要設為開機自動啟動（`sudo systemctl enable docker`）。容器使用 `restart: always`，主機重開機後會跟著 Docker 自動啟動。
- 防火牆對區網開放 TCP 3307（MCP）與 TCP 65000–65100（遊戲埠）。RCON 只在容器內使用，不需要也不應開放。
- 容器需要能連外網：Paper 第一次開機時會下載 Mojang 伺服器檔。

## 安裝

```sh
./setup.sh
```

`setup.sh` 會依序完成：

1. 顯示 Minecraft EULA 說明。**本程式建立的每個實例都會自動接受 EULA**，必須輸入 `yes` 同意才會繼續。
2. 詢問 TLS 憑證要包含的 IP／主機名稱。
3. 寫入 `.env`（`ABMPT_TLS_SANS`、`ABMPT_EULA_ACCEPTED=true`、同意時間與帳號）。
4. 建立 `data/`，並把擁有者設為 uid 10001（容器內以非 root 身分執行）。
5. 執行 `docker compose up -d --build`。

`ABMPT_EULA_ACCEPTED` 不是 `true` 時，建立實例會被拒絕。

### 手動安裝

不使用 `setup.sh` 時，自行建立 `data/`（`sudo chown 10001:10001 data`）與 `.env`。只有在閱讀並同意 EULA 後，才能設定 `ABMPT_EULA_ACCEPTED=true`。

可調整的環境變數（寫在 `.env` 或 `compose.yaml`）：

| 環境變數 | 預設 | 說明 |
|---|---|---|
| `ABMPT_TLS_SANS` | `localhost,127.0.0.1` | 用戶端連線時使用的所有 IP／主機名稱，以逗號分隔。會寫進伺服器憑證 |
| `ABMPT_EULA_ACCEPTED` | `false` | 操作者已同意 Minecraft EULA；設為 `true` 後，新實例會自動接受 EULA |
| `ABMPT_GAME_PORTS` | `65000-65100` | 遊戲埠範圍，要與 `ports:` 一致 |
| `ABMPT_RCON_PORT_BASE` | `35000` | 容器內 RCON 埠起點（不對外） |
| `ABMPT_BOOT_TIMEOUT_SECS` | `600` | 開機逾時秒數 |
| `ABMPT_STOP_TIMEOUT_SECS` | `90` | 正常關機等待秒數，逾時才強制結束 |
| `ABMPT_AUTOSTART_DELAY_SECS` | `5` | 自動啟動時，各實例之間的間隔 |
| `ABMPT_MAX_UPLOAD_MB` | `512` | 單一上傳檔案上限 |

`stop_grace_period` 必須大於 `ABMPT_STOP_TIMEOUT_SECS`，才能在容器停止時讓所有伺服器存檔。

```sh
docker compose up -d --build
docker compose logs -f
```

## token 管理

```sh
docker exec abmpt abmpt token create claude   # 建立，明文只印一次
docker exec abmpt abmpt token list
docker exec abmpt abmpt token revoke claude   # 立即生效，不影響其他 token
```

伺服器只保存 token 的 SHA-256 雜湊（`data/tokens.json`）。要換發 token 時，先 revoke 再 create。

## TLS 憑證

第一次啟動時，服務會在 `data/tls/` 產生：

- `ca.crt`／`ca.key`：本地 CA（有效 20 年）。`ca.key` 不要外流。
- `server.crt`／`server.key`：伺服器憑證，涵蓋 `ABMPT_TLS_SANS`。修改 SAN 或接近到期時會自動重新簽發，CA 不變，所以用戶端不需要重新設定。

把 `ca.crt` 發給各台用戶端機器，設定方式見 [CLIENT-SETUP.md](CLIENT-SETUP.md)。也可以用 `docker exec abmpt abmpt ca` 印出內容。

## 資料目錄

`data/` 包含所有狀態，備份或搬家時複製整個目錄即可：

| 路徑 | 內容 |
|---|---|
| `instances/<name>/` | 各實例的伺服器資料夾（世界、插件、設定、log） |
| `meta/<name>.json` | 實例設定（含 RCON 密碼，權限 600） |
| `jars/` | 伺服器核心 jar |
| `uploads/` | 暫存上傳檔，24 小時後清除 |
| `backups/<name>/` | 設定檔與插件的舊版本備份；刪除實例時一併刪除 |
| `audit/audit.jsonl` | 稽核紀錄 |
| `tokens.json` | token 雜湊 |
| `tls/` | CA 與伺服器憑證 |

## 搬到其他主機

1. 在舊主機上執行 `docker compose down`，等待所有伺服器存檔。
2. 把整個專案目錄（含 `data/`）複製到新主機，保留檔案擁有者（例如 `sudo rsync -a`）。
3. 如果 IP 或主機名稱改變，就修改 `ABMPT_TLS_SANS`。
4. 執行 `docker compose up -d --build`。

## 稽核紀錄

`data/audit/audit.jsonl` 每行是一筆 JSON，包含 `ts`、`agent`、`instance`、`action`、`detail`、`ok`、`result`。檔案內容不會出現 token、RCON 密碼或寫入設定檔的內容。
