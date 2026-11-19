# 部署目錄與 setup.sh

日期：2026-11-20。使用者指示「安裝腳本可以參照 https://github.com/Sparxilium-Network/GitStatus」（其 `deploy/setup.sh`、`deploy/docker-compose.yml`）。

## 沿用的做法
- 部署檔放在 `deploy/`（compose 的 build context 為 `..`），資料放在 `deploy/data/`。
- 不是 root 時用 `exec sudo -- bash "$SCRIPT"` 重新執行；`SERVICE_UID=10001`；輸出函式 `bold/ok/warn/fail`、`ask_yes`。
- 模式：無參數（新安裝 → setup；已安裝 → 選單 1) Upgrade 2) Setup），以及 `upgrade`（不詢問、不執行 git pull）。
- 設定檔從範本建立並帶版本號（`ABMPT_CONFIG_VERSION`），以 while/case 迴圈升級，版本比腳本新時拒絕執行。
- 統一修正擁有者與權限，啟動後輪詢健康檢查 90 秒，失敗時印出 `compose logs --tail=40` 並以 exit 1 結束。
- 可重複執行，按 Enter 保留原值。

## 與參考不同之處與理由
- 設定檔是 `deploy/.env` 而不是 `config/config.conf`：compose 只會從 `.env` 讀取 `${ABMPT_BIND_IP}` 這類插值，並透過 `env_file` 傳給容器。檔案不含機密，權限 644。
- 參考專案綁 `127.0.0.1`（透過 tunnel 對外）；本服務需求是對區網開放，因此改用 `ABMPT_BIND_IP`，預設為偵測到的區網 IP，並在腳本與文件中說明 Docker 發佈的埠會略過 ufw。
- 健康檢查走 HTTPS：`curl --cacert data/tls/ca.crt --resolve localhost:3307:<bind>`，用 SNI localhost 驗證（SAN 一定包含 localhost／127.0.0.1，腳本會自動補上）。
- 額外步驟：EULA 同意（第 1 步，只在尚未同意時詢問）、`systemctl enable docker`（驗收第 10 項需要）、啟動後把 `ca.crt` 複製到 `deploy/ca.crt`（`data/` 為 700，一般使用者讀不到）、在 setup 模式中建立 token（明文只顯示一次）。
- 重啟策略維持 `restart: always`（需求要求開機自動啟動），不設 `mem_limit`（需求為不設限）。
