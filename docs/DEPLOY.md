# 部署與維運

> 最後修改：2026-11-20 ｜ 基於 commit：`53199fc`

## 前置需求

- Linux 主機，已安裝 Docker Engine 與 Compose plugin。
- 防火牆對區網開放 TCP 3307（MCP）與 TCP 65000–65100（遊戲埠）。RCON 只在容器內使用，不需要也不應開放。
- 容器需要能連外網：Paper／Folia 第一次開機時會下載 Mojang 伺服器檔。

## 安裝

```sh
git clone <repo> abmpt
cd abmpt/deploy
sudo ./setup.sh
```

腳本不是用 root 執行時，會自動改用 sudo 重新執行。步驟如下：

1. **Minecraft EULA**：說明本程式建立的每一台實例都會**自動接受 [EULA](https://aka.ms/MinecraftEULA)**，必須輸入 `yes` 同意才會繼續。同意的時間與帳號會記錄在 `.env`。
2. **網路**：
   - 選擇埠要發佈在哪個主機位址。預設是偵測到的區網 IP。
   - 選擇連線方式：HTTPS（預設，自簽 CA）或純 HTTP（見下方[純 HTTP 模式](#純-http-模式)）。選純 HTTP 時要再確認一次。
   - 輸入主機名稱，也就是 AI 用戶端連線時使用的 IP 或主機名稱。用其他名稱連線會被拒絕；HTTPS 模式下也是憑證上的名稱。`localhost` 與 `127.0.0.1` 會自動加入。
3. **開機自動啟動**：若 Docker 服務還沒有設為開機啟動，會詢問是否執行 `systemctl enable docker`。
4. **擁有者與權限**：`data/` 改為 uid 10001 所有、權限 700。

接著腳本會建置並啟動容器，等待 `/healthz` 就緒，把 CA 憑證複製到 `deploy/ca.crt`（只有 HTTPS 模式），最後可以直接為各個 AI 建立 token。

腳本可以重複執行：按 Enter 會保留原本的值，同時重新修正權限。

> **Docker 發佈的埠會略過 ufw／firewalld。** 綁定 `0.0.0.0` 時，所有網路介面都能連到這兩組埠。建議只綁定區網 IP。

## 升級

```sh
cd abmpt
git pull
cd deploy
sudo ./setup.sh upgrade
```

`upgrade` 模式不會詢問任何問題，會依序：

- 保留所有設定
- 將 `.env` 升級到新版格式（若有需要）
- 修正權限
- 重新建置並重啟容器

在已安裝的主機上直接執行 `sudo ./setup.sh` 時，第一個問題可以選 Upgrade 或 Setup（修改設定）。

重啟容器時，所有伺服器會先正常存檔關機；設為自動啟動的實例之後會再依序開機。

## 設定（`deploy/.env`）

`setup.sh` 會從 `.env.example` 建立 `.env`。這個檔案不含機密，也可以手動修改，改完執行 `sudo ./setup.sh upgrade` 讓設定生效。

| 變數 | 預設 | 說明 |
|---|---|---|
| `ABMPT_CONFIG_VERSION` | `2` | 檔案格式版本，由 setup.sh 管理 |
| `ABMPT_BIND_IP` | `0.0.0.0` | 發佈 3307 與遊戲埠的主機位址 |
| `ABMPT_TLS` | `on` | `on`＝HTTPS（自簽 CA）；`off`＝純 HTTP，未加密 |
| `ABMPT_TLS_SANS` | `localhost,127.0.0.1` | 允許連線使用的 IP／主機名稱；HTTPS 模式下也寫進伺服器憑證 |
| `ABMPT_EULA_ACCEPTED` | `false` | 操作者已同意 EULA。不是 `true` 時，建立實例會被拒絕 |
| `ABMPT_EULA_ACCEPTED_AT` | | 同意的時間與帳號 |
| `ABMPT_BOOT_TIMEOUT_SECS` | `600` | 開機逾時秒數 |
| `ABMPT_STOP_TIMEOUT_SECS` | `90` | 正常關機的等待秒數，逾時才強制結束。必須小於 compose 的 `stop_grace_period`（3 分鐘） |
| `ABMPT_AUTOSTART_DELAY_SECS` | `5` | 自動啟動時，各實例之間的間隔 |
| `ABMPT_MAX_UPLOAD_MB` | `512` | 單一上傳檔案的大小上限 |

遊戲埠範圍固定為 65000–65100（同時寫在 `docker-compose.yml` 與映像檔中）。

## token 管理

```sh
docker exec abmpt abmpt token create claude   # 建立，明文只印一次
docker exec abmpt abmpt token list
docker exec abmpt abmpt token revoke claude   # 立即生效，不影響其他 token
```

伺服器只保存 token 的 SHA-256 雜湊（`data/tokens.json`）。要換發 token 時，先 revoke 再 create。

## TLS 憑證

服務第一次啟動時，會在 `data/tls/` 產生下列檔案：

- `ca.crt`／`ca.key`：本地 CA，有效 20 年。`ca.key` 不要外流。
- `server.crt`／`server.key`：伺服器憑證，涵蓋 `ABMPT_TLS_SANS`。修改名稱清單或接近到期時，會自動重新簽發；CA 不變，所以用戶端不需要重新設定。

`setup.sh` 會把 `ca.crt` 複製到 `deploy/ca.crt`。這是公開資訊，權限為 644，請發給各台 AI 用戶端。也可以用 `docker exec abmpt abmpt ca` 印出內容。用戶端的設定方式見 [CLIENT-SETUP.md](CLIENT-SETUP.md)。

## 純 HTTP 模式

`ABMPT_TLS=off` 時，服務改用 `http://<主機>:3307`，不產生也不使用任何憑證，用戶端不需要信任 CA。

> **代價：** token、上傳的 jar、讀寫的設定檔內容（包括 AI 寫入的資料庫密碼）都以明文在網路上傳送。同一網段的人可以側錄 token，拿到之後就能完全控制測試伺服器。只應在受信任的開發用網路中使用。

- 服務啟動時會在 log 印出警告。
- 主機名稱清單（`ABMPT_TLS_SANS`）仍然有效：用不在清單內的名稱連線會被拒絕（HTTP 403）。
- 從 HTTPS 切到 HTTP 時，`data/tls/` 會保留；之後切回 HTTPS 會沿用同一個 CA，用戶端不需要重新設定。
- 切換方式：重新執行 `sudo ./setup.sh` 並選 Setup，或修改 `.env` 後執行 `sudo ./setup.sh upgrade`。

## 資料目錄（`deploy/data/`）

所有狀態都放在這個目錄，備份或搬家時複製整個目錄即可。

| 路徑 | 內容 |
|---|---|
| `instances/<name>/` | 各實例的伺服器資料夾（世界、插件、設定、log） |
| `meta/<name>.json` | 實例設定，含 RCON 密碼（權限 600） |
| `jars/` | 伺服器核心 jar |
| `uploads/` | 暫存的上傳檔，24 小時後清除 |
| `backups/<name>/` | 設定檔與插件的舊版本備份。刪除實例時一併刪除 |
| `audit/audit.jsonl` | 稽核紀錄 |
| `tokens.json` | token 的雜湊 |
| `tls/` | CA 與伺服器憑證 |

## 搬到其他主機

1. 在舊主機的 `deploy/` 中執行 `docker compose down`，等所有伺服器存檔關機。
2. 把整個專案目錄（含 `deploy/data/` 與 `deploy/.env`）複製到新主機，並保留檔案擁有者，例如使用 `sudo rsync -a`。
3. 在新主機執行 `sudo ./setup.sh`，選 Setup，更新綁定位址與主機名稱。

## 稽核紀錄

`data/audit/audit.jsonl` 每一行是一筆 JSON，欄位如下：

- `ts`：時間
- `agent`：依 token 分辨的 AI
- `instance`：操作的實例
- `action`：做了什麼
- `detail`：參數摘要
- `ok`：是否成功
- `result`：結果摘要

紀錄中不會出現 token、RCON 密碼，或寫入設定檔的內容。
