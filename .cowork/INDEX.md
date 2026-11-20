# .cowork 索引

| 檔案 | 摘要 | 關鍵字 |
|------|------|--------|
| `decisions/initial-architecture.md` | 語言 Rust、同埠 HTTP 上傳、自簽 CA TLS、容器內 CLI 管 token、bridge 網路、初始化綁 127.0.0.1、EULA 改由 setup.sh 取得同意 | architecture, rust, tls, token, upload, docker, network, init, eula |
| `decisions/deploy-setup-script.md` | deploy/ 目錄與 setup.sh 參照 GitStatus 的做法、刻意不同之處（.env、BIND_IP、HTTPS 健康檢查），以及可選的純 HTTP 模式 | deploy, setup.sh, install, upgrade, env, compose, http, tls |
| `verification/e2e-local-docker.md` | 本機 Docker 依需求書 11 項驗收的結果、修正的問題、尚未驗證的項目 | e2e, acceptance, docker, folia, luckperms, test |
| `verification/setup-script.md` | setup.sh 替身測試（含純 HTTP 情境共 49 項）與 HTTP 模式的實際容器測試 | setup.sh, test, install, upgrade, http |
