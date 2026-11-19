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
