# 設定 AI 用戶端

> 最後修改：2026-11-20 ｜ 基於 commit：`bbbe96b`

以下以 `192.168.75.206` 為主機位址。每台用戶端機器需要準備：

1. **信任 CA**：取得伺服器上的 `deploy/ca.crt`（由 `setup.sh` 複製出來），例如執行 `scp user@192.168.75.206:<專案路徑>/deploy/ca.crt .`。
2. **token**：放在環境變數 `ABMPT_TOKEN`。不要寫進任何會進版本控制的檔案，也不要貼進對話。
3. **工具逾時**：開機與初始化會等伺服器真正開完機才回傳。第一次啟動 Paper 可能要好幾分鐘，建議把 MCP 工具逾時設為 15 分鐘以上。

## 信任 CA

建議直接加入作業系統的信任儲存區，所有用戶端都能使用：

```sh
# Debian / Ubuntu
sudo cp ca.crt /usr/local/share/ca-certificates/abmpt-ca.crt && sudo update-ca-certificates
```

```powershell
# Windows（系統管理員）
certutil -addstore -f Root ca.crt
```

```sh
# macOS
sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain ca.crt
```

以 Node.js 實作的用戶端，也可以只對該程式設定 `NODE_EXTRA_CA_CERTS=/path/to/ca.crt`。

## Claude Code

```sh
claude mcp add --transport http abmpt https://192.168.75.206:3307/mcp \
  --header 'Authorization: Bearer ${ABMPT_TOKEN}'
```

也可以寫進專案的 `.mcp.json`（`${ABMPT_TOKEN}` 會在執行時從環境變數展開）：

```json
{
  "mcpServers": {
    "abmpt": {
      "type": "http",
      "url": "https://192.168.75.206:3307/mcp",
      "headers": { "Authorization": "Bearer ${ABMPT_TOKEN}" }
    }
  }
}
```

工具逾時：設定環境變數 `MCP_TOOL_TIMEOUT=900000`（毫秒）。

## Codex CLI

`~/.codex/config.toml`：

```toml
[mcp_servers.abmpt]
url = "https://192.168.75.206:3307/mcp"
bearer_token_env_var = "ABMPT_TOKEN"
tool_timeout_sec = 900
```

## Gemini CLI

`~/.gemini/settings.json`：

```json
{
  "mcpServers": {
    "abmpt": {
      "httpUrl": "https://192.168.75.206:3307/mcp",
      "headers": { "Authorization": "Bearer $ABMPT_TOKEN" },
      "timeout": 900000
    }
  }
}
```

> 各用戶端的設定鍵名可能隨版本調整，請以該工具的官方文件為準。

## 上傳 jar

MCP 工具參數無法承載大型檔案，因此 jar 檔要先用 HTTPS 上傳，再把回傳的 `upload_id` 交給工具使用：

```sh
curl --fail --cacert ca.crt \
  -H "Authorization: Bearer $ABMPT_TOKEN" \
  --upload-file LuckPerms-Bukkit-5.4.jar \
  "https://192.168.75.206:3307/upload?filename=LuckPerms-Bukkit-5.4.jar"
```

已把 CA 加入系統信任時，可以省略 `--cacert`。回傳的 JSON 含 `upload_id`（24 小時內有效），之後交給 `core_jar_import`（伺服器核心）或 `plugin_deploy`（插件）使用。
