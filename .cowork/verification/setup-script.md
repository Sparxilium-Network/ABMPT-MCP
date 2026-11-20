# setup.sh 測試

日期：2026-11-20。方法：在拋棄式 Debian 13 容器（root）中，以替身的 `docker`／`curl`／`hostname`／`systemctl` 執行 `deploy/setup.sh`，並用 stdin 模擬輸入。沒有實際建置或啟動容器。

結果：7 個情境、32 項檢查全部通過。
1. 新安裝時拒絕 EULA → 非 0 結束，`.env` 仍為 false，沒有啟動服務。
2. 新安裝時同意並全部使用預設 → EULA true 並記錄時間與帳號、綁定區網 IP、SAN 含區網 IP／localhost／127.0.0.1、執行 systemctl enable、data 為 10001:700、.env 為 644、ca.crt 為 644、執行 compose up、token 建立且只顯示一次、版本號維持 1。
3. 已安裝時重跑並選預設的 Upgrade → 不再詢問 EULA、重啟服務、沒有 token 步驟。
4. `upgrade` 參數加上 stdin 為 /dev/null → 成功並重啟。
5. 重跑並選 Setup，綁定 0.0.0.0、自訂 SAN（不含 localhost）→ 自動補上 localhost 與 127.0.0.1；名稱重複的 token 被拒絕；EULA 那一行不會重複。
6. 新目錄執行 `upgrade` → 被拒絕。
7. 健康檢查一直失敗 → 印出 compose logs 並以 exit 1 結束。

另外：`bash -n` 語法檢查通過；`docker compose config` 確認 `.env` 的 `ABMPT_BIND_IP` 有套用到 3307 與遊戲埠的 host_ip。

未測：真實 Linux 主機上的完整執行（真實的 sudo、systemd、docker build、TLS 健康檢查）。shellcheck 未執行（本機沒有安裝）。

## 純 HTTP 模式（2026-11-20）
替身測試新增情境，總計 49 項全部通過：選 HTTP 並確認 → `ABMPT_TLS=off`、健康檢查走 http、不複製 ca.crt、顯示警告與 http 網址；再跑 Setup 切回 HTTPS → 選單顯示目前為 HTTP、改回 on、ca.crt 複製；選 HTTP 但不確認 → 維持 on；第 1 版 `.env` 執行 upgrade → 升到第 2 版並補上 `ABMPT_TLS=on`。
實際容器（`ABMPT_TLS=off`）：`/healthz` 走 http 正常、https 連線被拒、無 token 回 401、MCP initialize 與 `server_info` 的上傳說明為 http 且沒有 `--cacert`、錯誤 Host 回 403、上傳端點可用、`abmpt ca` 回報錯誤、log 中沒有 token。預設模式重測：https 正常、純 http 被拒、沒有 HTTP 警告。Windows 的 curl（Schannel）對自簽 CA 會報撤銷狀態未知，需加 `--ssl-no-revoke`，已寫進 CLIENT-SETUP.md。
