#!/usr/bin/env bash
# Interactive setup for this deploy directory:
#   Minecraft EULA consent (every instance this service creates accepts the EULA automatically)
#   .env (created from .env.example, upgraded to the current ABMPT_CONFIG_VERSION)
#   network: published host address and TLS certificate names
#   Docker started at boot, ownership and permissions of data/ (the container runs as uid 10001)
#   starts the service, waits for /healthz, copies the CA certificate, optionally creates tokens
#
# Usage, in the deploy directory:  sudo ./setup.sh
#                                   sudo ./setup.sh upgrade   (after git pull: no questions)
# Safe to re-run: press Enter to keep existing values (also fixes permissions).
# On an existing installation the first question offers "Upgrade": keep every setting, fix
# permissions and rebuild/restart the service. It does not run git pull.
set -euo pipefail

SERVICE_UID=10001
PORT=3307
SERVICE=abmpt
SCRIPT=$(readlink -f "$0")
cd "$(dirname "$SCRIPT")"

if [ "$(id -u)" -ne 0 ]; then
  echo "setup.sh 需要 root 權限來設定檔案擁有者，改用 sudo 重新執行..."
  exec sudo -- bash "$SCRIPT" "$@"
fi

ENV_FILE=.env
ENV_EXAMPLE=.env.example
# .env format version this script writes
CONFIG_VERSION=1
MODE=
case ${1:-} in
  "") ;;
  upgrade) MODE=upgrade ;;
  *) echo "用法：sudo ./setup.sh [upgrade]" >&2; exit 2 ;;
esac

bold() { printf '\n\033[1m%s\033[0m\n' "$*"; }
ok()   { printf '  \033[32mOK\033[0m  %s\n' "$*"; }
warn() { printf '  \033[33m!!\033[0m  %s\n' "$*"; }
fail() { printf '  \033[31mERROR\033[0m  %s\n' "$*" >&2; exit 1; }

# ask_yes "Question" Y|N  -> exit status 0 for yes
ask_yes() {
  local answer hint="[y/N]"
  [ "$2" = Y ] && hint="[Y/n]"
  read -rp "  $1 $hint " answer
  answer=${answer:-$2}
  [[ $answer =~ ^[Yy] ]]
}

# get_env KEY -> value of KEY in .env (last definition, surrounding quotes removed)
get_env() {
  local v
  v=$(sed -n "s/^$1=//p" "$ENV_FILE" | tail -n 1)
  v=${v%\"}; v=${v#\"}
  printf '%s' "$v"
}

# set_env KEY VALUE: replaces KEY in .env, or appends it
set_env() {
  local esc
  if grep -q "^$1=" "$ENV_FILE"; then
    esc=$(printf '%s' "$2" | sed 's/[\\|&]/\\&/g')
    sed -i "s|^$1=.*|$1=$esc|" "$ENV_FILE"
  else
    printf '%s=%s\n' "$1" "$2" >> "$ENV_FILE"
  fi
}

lan_ips() {
  hostname -I 2>/dev/null | tr ' ' '\n' | grep -E '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$' || true
}

compose() { docker compose "$@"; }

command -v docker >/dev/null || fail "找不到 docker。請先安裝 Docker Engine（含 Compose plugin）。"
docker compose version >/dev/null 2>&1 || fail "找不到 docker compose plugin。"
[ -f "$ENV_EXAMPLE" ] || fail "找不到 $ENV_EXAMPLE。請在專案的 deploy 目錄執行本腳本。"
[ -f ../Dockerfile ] || fail "找不到 ../Dockerfile。請在專案的 deploy 目錄執行本腳本。"
mkdir -p data

# ---- .env: create it, or upgrade it to CONFIG_VERSION -----------------------
# A release that adds or renames settings raises CONFIG_VERSION and adds a step below that
# edits existing files in place.
if [ -d "$ENV_FILE" ]; then
  fail "$ENV_FILE 是一個目錄。請移除後重新執行。"
fi
if [ ! -f "$ENV_FILE" ]; then
  install -m 644 "$ENV_EXAMPLE" "$ENV_FILE"
  ok "已從 $ENV_EXAMPLE 建立 $ENV_FILE"
fi
config_version=$(get_env ABMPT_CONFIG_VERSION | tr -d '[:space:]')
if [ -z "$config_version" ]; then
  sed -i '1i # Format version of this file; setup.sh upgrades older files. Do not change it by hand.\nABMPT_CONFIG_VERSION=1\n' "$ENV_FILE"
  config_version=1
  ok "已在 $ENV_FILE 加入 ABMPT_CONFIG_VERSION=1"
fi
[[ $config_version =~ ^[0-9]+$ ]] || fail "$ENV_FILE 的 ABMPT_CONFIG_VERSION 是 '$config_version'，不是數字。"
if [ "$config_version" -gt "$CONFIG_VERSION" ]; then
  fail "$ENV_FILE 是第 $config_version 版，比這個 setup.sh 認得的版本（$CONFIG_VERSION）新。請先 git pull。"
fi
while [ "$config_version" -lt "$CONFIG_VERSION" ]; do
  case $config_version in
    # 1) ... edit settings for version 2 ... ;;
    *) fail "沒有從 ABMPT_CONFIG_VERSION=$config_version 升級的步驟。" ;;
  esac
  config_version=$((config_version + 1))
  set_env ABMPT_CONFIG_VERSION "$config_version"
  ok "已將 $ENV_FILE 升級到第 $config_version 版"
done

# installed -> exit status 0 when EULA consent and network settings are in place
installed() {
  [ "$(get_env ABMPT_EULA_ACCEPTED)" = true ] && [ -n "$(get_env ABMPT_TLS_SANS)" ] && [ -d data/tls ]
}

# ---- Upgrade or setup --------------------------------------------------------
if installed; then
  if [ -z "$MODE" ]; then
    bold "偵測到既有安裝"
    echo "  綁定位址：$(get_env ABMPT_BIND_IP)"
    echo "  TLS 名稱：$(get_env ABMPT_TLS_SANS)"
    echo "  EULA：已於 $(get_env ABMPT_EULA_ACCEPTED_AT) 同意"
    cat <<'EOF'

    1) Upgrade  保留所有設定，修正權限並重新建置／重啟服務
                （請先自行 git pull，本腳本不會執行）
                重啟時會先讓所有伺服器正常存檔關機，設為自動啟動的實例會再自動開機
    2) Setup    檢視或修改設定，並可建立 token
EOF
    while [ -z "$MODE" ]; do
      read -rp "  請選擇 [1]: " choice
      case ${choice:-1} in
        1) MODE=upgrade ;;
        2) MODE=setup ;;
        *) warn "請輸入 1 或 2。" ;;
      esac
    done
  fi
elif [ "$MODE" = upgrade ]; then
  fail "尚未安裝，無法升級（EULA 尚未同意或沒有 TLS 設定）。請執行 sudo ./setup.sh（不加 upgrade）。"
else
  MODE=setup
fi

if [ "$MODE" = setup ]; then
# ---- 1. Minecraft EULA -------------------------------------------------------
bold "1/4  Minecraft EULA"
if [ "$(get_env ABMPT_EULA_ACCEPTED)" = true ]; then
  ok "已於 $(get_env ABMPT_EULA_ACCEPTED_AT) 同意 EULA"
else
  cat <<'EOF'
  本服務會由 AI 自動建立並啟動 Minecraft（Paper／Folia）伺服器。
  使用本程式時，每一台新建立的伺服器實例都會【自動接受】
  Minecraft End User License Agreement（EULA）：

      https://aka.ms/MinecraftEULA

  AI 建立實例時不會再逐一詢問你。繼續安裝即代表你已閱讀並同意
  Minecraft EULA，並同意本程式代表你為之後建立的所有實例接受 EULA。
EOF
  echo
  read -rp "  若同意，請輸入 yes：" answer
  [ "$answer" = yes ] || fail "未同意 EULA，安裝中止。"
  set_env ABMPT_EULA_ACCEPTED true
  set_env ABMPT_EULA_ACCEPTED_AT "$(date -u +%Y-%m-%dT%H:%M:%SZ)/${SUDO_USER:-root}@$(hostname)"
  ok "已記錄同意（$ENV_FILE）"
fi

# ---- 2. Network ---------------------------------------------------------------
bold "2/4  網路"
mapfile -t ips < <(lan_ips)
default_ip=${ips[0]:-}
cat <<EOF
  MCP 埠 $PORT 與遊戲埠 65000-65100 要發佈在主機的哪個位址。
  注意：Docker 發佈的埠會略過 ufw／firewalld，建議只綁在區網 IP，而不是 0.0.0.0。
  這台主機的 IP：${ips[*]:-（偵測不到）}
EOF
cur_bind=$(get_env ABMPT_BIND_IP)
[ "$cur_bind" = 0.0.0.0 ] && [ -n "$default_ip" ] && cur_bind=$default_ip
while true; do
  read -rp "  綁定位址 [${cur_bind:-0.0.0.0}]: " bind
  bind=${bind:-${cur_bind:-0.0.0.0}}
  if [[ ! $bind =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    warn "請輸入 IPv4 位址，例如 192.168.75.206 或 0.0.0.0。"
  elif [ "$bind" != 0.0.0.0 ] && ! printf '%s\n' "${ips[@]}" | grep -qx "$bind"; then
    warn "$bind 不是這台主機的 IP。"
    ask_yes "仍要使用？" N && break
  else
    break
  fi
done
set_env ABMPT_BIND_IP "$bind"
ok "綁定位址：$bind"

echo
cat <<'EOF'
  TLS 憑證名稱：AI 用戶端連線時使用的所有 IP 或主機名稱，以逗號分隔。
  用戶端用不在清單內的名稱連線時，TLS 驗證會失敗。localhost 與 127.0.0.1 會自動加入。
EOF
cur_sans=$(get_env ABMPT_TLS_SANS)
if [ "$cur_sans" = "localhost,127.0.0.1" ] || [ -z "$cur_sans" ]; then
  suggest=$bind
  [ "$suggest" = 0.0.0.0 ] && suggest=$default_ip
  cur_sans=${suggest:+$suggest,}localhost,127.0.0.1
fi
read -rp "  TLS 名稱 [$cur_sans]: " sans
sans=${sans:-$cur_sans}
sans=$(printf '%s' "$sans" | tr -d ' ')
for must in localhost 127.0.0.1; do
  printf '%s' ",$sans," | grep -q ",$must," || sans="$sans,$must"
done
set_env ABMPT_TLS_SANS "$sans"
ok "TLS 名稱：$sans（變更後，伺服器憑證會在下次啟動時重新簽發，CA 不變）"

# ---- 3. Docker at boot ---------------------------------------------------------
bold "3/4  開機自動啟動"
if command -v systemctl >/dev/null && systemctl list-unit-files docker.service >/dev/null 2>&1; then
  if systemctl is-enabled --quiet docker; then
    ok "Docker 服務已設為開機啟動；容器使用 restart: always，會跟著啟動"
  elif ask_yes "Docker 服務尚未設為開機啟動，現在設定？" Y; then
    systemctl enable docker >/dev/null 2>&1
    ok "已執行 systemctl enable docker"
  else
    warn "主機重開機後，服務不會自動上線"
  fi
else
  warn "找不到 systemd 的 docker.service；請自行確認 Docker 會在開機時啟動"
fi
fi # MODE=setup

# ---- 4. Ownership and permissions ------------------------------------------
if [ "$MODE" = setup ]; then
  bold "4/4  擁有者與權限（容器使用者 uid $SERVICE_UID）"
else
  bold "擁有者與權限（容器使用者 uid $SERVICE_UID）"
fi
chown -R "$SERVICE_UID:$SERVICE_UID" data
chmod 700 data
chmod 644 "$ENV_FILE"
ok "data/ 屬於 uid $SERVICE_UID（chmod 700），$ENV_FILE 為 644（不含機密）"

# ---- Start -----------------------------------------------------------------
health_host=$(get_env ABMPT_BIND_IP)
[ "$health_host" = 0.0.0.0 ] && health_host=127.0.0.1
health() {
  [ -s data/tls/ca.crt ] || return 1
  if command -v curl >/dev/null; then
    curl -fsS --cacert data/tls/ca.crt --resolve "localhost:$PORT:$health_host" \
      "https://localhost:$PORT/healthz" 2>/dev/null | grep -q ok
  else
    (exec 3<>"/dev/tcp/$health_host/$PORT") 2>/dev/null
  fi
}

echo
if [ "$MODE" = setup ] && ! ask_yes "現在啟動（或重啟）服務？" Y; then
  echo
  echo "  之後可以執行：docker compose up -d --build --force-recreate"
  exit 0
fi
compose up -d --build --force-recreate
printf '  等待 https://%s:%s/healthz ' "$health_host" "$PORT"
healthy=
for _ in $(seq 1 45); do
  if health; then
    healthy=1
    break
  fi
  printf '.'
  sleep 2
done
echo
if [ -z "$healthy" ]; then
  warn "服務在 90 秒內沒有就緒。最近的 log："
  compose logs --tail=40 "$SERVICE"
  exit 1
fi
ok "服務已啟動"
install -m 644 data/tls/ca.crt ca.crt
ok "CA 憑證已複製到 $PWD/ca.crt（公開資訊，可以發給 AI 用戶端）"
[ "$MODE" = setup ] || exit 0

# ---- Tokens ------------------------------------------------------------------
bold "AI token"
cat <<'EOF'
  每個 AI 各用一組 token，權限相同，可以單獨撤銷。
  明文只會顯示這一次：請直接設定到該 AI 所在機器的環境變數 ABMPT_TOKEN，不要貼進對話或存進 git。
EOF
existing=$(compose exec -T "$SERVICE" abmpt token list 2>/dev/null || true)
if [ -n "$existing" ]; then
  echo
  echo "  已有的 token："
  printf '%s\n' "$existing" | sed 's/^/    /'
fi
echo
while ask_yes "現在為某個 AI 建立 token？" "$([ -z "$existing" ] && echo Y || echo N)"; do
  read -rp "  名稱（例如 claude、codex、gemini）: " name
  if [[ ! $name =~ ^[a-z0-9][a-z0-9_-]{0,31}$ ]]; then
    warn "名稱只能使用 a-z、0-9、'_'、'-'，最多 32 個字元。"
    continue
  fi
  if token=$(compose exec -T "$SERVICE" abmpt token create "$name" 2>/dev/null); then
    echo
    echo "  $name 的 token（只顯示這一次）："
    echo
    echo "      $token"
    echo
    unset token
    read -rp "  記下後按 Enter 繼續... " _
  else
    warn "建立失敗（名稱可能已存在；要換發請先 docker exec $SERVICE abmpt token revoke $name）。"
  fi
  existing=1
  echo
done

server=${health_host/127.0.0.1/$(hostname)}
echo
echo "  下一步："
echo "    - 在每台 AI 用戶端機器取得 CA 憑證並設定信任："
echo "        scp ${SUDO_USER:-user}@${server}:$PWD/ca.crt ."
echo "    - 設定 AI 用戶端：../docs/CLIENT-SETUP.md"
echo "    - 之後要新增或撤銷 token：docker exec $SERVICE abmpt token create|revoke <名稱>"
