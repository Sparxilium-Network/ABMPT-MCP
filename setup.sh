#!/usr/bin/env bash
# ABMPT installer: Minecraft EULA consent, data folder, TLS names, start the container.
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '%s\n' "$*"; }

command -v docker >/dev/null || { say "Docker 未安裝。請先安裝 Docker Engine 與 Compose plugin。"; exit 1; }
docker compose version >/dev/null || { say "找不到 docker compose plugin。"; exit 1; }

cat <<'EOF'

==================== Minecraft EULA ====================
本服務會自動建立並啟動 Minecraft（Paper／Folia）伺服器。
使用本程式時，每個新建立的伺服器實例都會【自動接受】
Minecraft End User License Agreement（EULA）：

    https://aka.ms/MinecraftEULA

AI 透過本服務建立實例時，不會再逐一詢問你。
繼續安裝即代表你已閱讀並同意 Minecraft EULA，
並同意本程式代表你為之後建立的所有實例接受 EULA。
========================================================

EOF
read -r -p '若同意，請輸入 yes：' answer
if [ "$answer" != "yes" ]; then
  say "未同意 EULA，安裝中止。"
  exit 1
fi
accepted_at="$(date -u +%Y-%m-%dT%H:%M:%SZ) by $(id -un)@$(hostname)"

default_ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
say ""
say "AI 用戶端連線時使用的 IP／主機名稱（會寫進 TLS 憑證，以逗號分隔）。"
read -r -p "SANs [${default_ip:+$default_ip,}localhost,127.0.0.1]：" sans
sans="${sans:-${default_ip:+$default_ip,}localhost,127.0.0.1}"

umask 077
cat > .env <<EOF
# Written by setup.sh — do not commit.
ABMPT_TLS_SANS=${sans}
ABMPT_EULA_ACCEPTED=true
ABMPT_EULA_ACCEPTED_AT=${accepted_at}
EOF
say "已寫入 .env"

mkdir -p data
if [ "$(stat -c %u data)" != "10001" ]; then
  say "設定 data/ 擁有者為 uid 10001（容器內執行身分），需要 sudo："
  sudo chown 10001:10001 data
fi

if command -v systemctl >/dev/null && ! systemctl is-enabled --quiet docker 2>/dev/null; then
  say "提醒：Docker 服務尚未設為開機啟動，請執行 sudo systemctl enable docker"
fi

docker compose up -d --build
say ""
say "服務已啟動：https://${sans%%,*}:3307/mcp"
say "CA 憑證：$(pwd)/data/tls/ca.crt（容器第一次啟動後產生，請發給各台 AI 用戶端）"
say "建立 token：docker exec abmpt abmpt token create <agent-name>"
