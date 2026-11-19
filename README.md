# Agent Based Minecraft Plugin Test（ABMPT）

> 最後修改：2026-11-20 ｜ 基於 commit：`53abdf6`

> [!WARNING]
> **請勿用於生產環境。** 本專案的定位類似 XAMPP，是方便在開發時使用的工具，安全性不足以對外提供服務。例如：使用自簽憑證、token 權限沒有分級、AI 可以執行任意主控台指令並修改設定檔，而且伺服器會自動接受 EULA。請只在受信任的開發用區網中使用，不要把埠開放到網際網路，也不要拿來運行正式的遊戲伺服器。

讓 AI（Claude Code、Codex CLI、Gemini CLI 等）透過 MCP 管理 Paper／Folia 測試伺服器的服務：建立、初始化、開關機、執行指令、看 log、部署插件、修改設定檔、刪除。RCON 密碼由服務自行保管，AI 不會碰到。

## 安裝

需要 Docker（含 Compose）。

```sh
git clone <repo> abmpt
cd abmpt/deploy
sudo ./setup.sh
```

> ⚠️ 本程式建立的每一台 Minecraft 伺服器都會**自動接受 [Minecraft EULA](https://aka.ms/MinecraftEULA)**。`setup.sh` 會先顯示這項說明，必須輸入 `yes` 同意才會繼續安裝。

`setup.sh` 會引導你完成設定、啟動服務，並為各個 AI 建立 token。完成後，把 `deploy/ca.crt` 複製到各台 AI 所在的機器並設定信任。

升級：先執行 `git pull`，再執行 `sudo ./setup.sh upgrade`。

詳細步驟：

- [部署與維運](docs/DEPLOY.md)
- [設定 AI 用戶端](docs/CLIENT-SETUP.md)

## 使用

AI 連上 `https://<主機>:3307/mcp` 後，呼叫 `server_info` 工具即可取得使用說明。jar 檔用 HTTPS `PUT /upload` 上傳。
