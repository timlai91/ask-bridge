# M365 Copilot Provider 實作規格

## 1. 文件狀態

- 狀態：Draft
- 目標版本：待排程
- 主要實作路徑：Microsoft 365 Copilot Chat 網頁 UI 自動化
- 後續選配路徑：Microsoft 365 Copilot Chat API
- 建議 CLI provider 名稱：`m365`
- 建議顯示名稱：`Microsoft 365 Copilot`

## 2. 背景與決策

`ask-bridge` 目前透過真實 Chrome、Chrome DevTools Protocol 與 `chrome-devtools-mcp` 自動操作 ChatGPT、Gemini 與 Claude。既有架構已將大部分 provider 差異集中在 `Provider` 的 URL、登入訊號、DOM selector、附件及模型選擇邏輯，因此新增 Microsoft 365 Copilot Chat 在技術上可行。

本規格採以下決策：

1. 第一階段沿用現有瀏覽器自動化架構，不引入 Microsoft Graph OAuth。
2. 第一個可發布版本標記為 experimental，僅承諾純文字提問與文字回覆。
3. `--model`、`--reasoning`、`--session`、`--image` 與 `--file` 必須先明確拒絕，不得以未驗證的通用 fallback 假裝支援。
4. 取得已授權的測試租戶並完成登入後 DOM 探勘，是進入正式實作前的必要閘門。
5. Microsoft 365 Copilot Chat API 保留為未來可選 transport，不阻塞 UI provider 上線。

## 3. 目標

完成後，使用者應能執行：

```powershell
ask-bridge --provider m365 login
ask-bridge --provider m365 "整理近期值得關注的 Rust 生態系發展。"
ask-bridge config --provider m365
ask-bridge --provider m365 --new "開始一個新的研究對話。"
```

系統必須：

- 使用 ask-bridge 專用 Chrome profile 保存 Microsoft Entra 登入狀態。
- 支援 Microsoft Entra 手動登入、MFA 與登入完成偵測。
- 自動開啟或重用 Microsoft 365 Copilot Chat 分頁。
- 將 prompt 寫入 composer 並送出。
- 等待新回覆完成後擷取最後一則 assistant 回覆。
- 將回覆以 Markdown 或可讀純文字輸出至終端機。
- 保留既有 ChatGPT、Gemini 與 Claude 行為。

## 4. 非目標

第一個版本不包含：

- Microsoft Graph 或 Copilot Chat API OAuth 整合。
- 存取郵件、會議、Teams、SharePoint 或其他組織資料的額外授權流程。
- 模型切換、推理強度切換或 Agent 選擇。
- 圖片產生、圖片下載及 Code Interpreter 類能力。
- 建立檔案、寄送郵件、建立會議等 Copilot action。
- 保證所有 Microsoft 365 租戶、國家雲及 Conditional Access 政策均可運作。
- 繞過 MFA、CAPTCHA、Conditional Access 或企業安全政策。

## 5. 先決條件

### 5.1 測試帳號

至少準備：

- 一個具 Copilot Chat 使用資格的 Microsoft 365 工作或學校帳號。
- 可完成 MFA 的測試人員。
- 至少一個英文 UI 與一個繁體中文 UI 測試環境；若無法取得兩者，第一版需記錄僅驗證的語系。
- 若要驗證 work grounding，另需具有 Microsoft 365 Copilot add-on 的帳號。

### 5.2 支援範圍

- 瀏覽器：Google Chrome。
- 入口網址：`https://m365.cloud.microsoft/chat`。
- 別名入口：`https://m365copilot.com`，可能重新導向 Microsoft 365 Copilot App。
- 登入提供者：Microsoft Entra ID。
- 初始平台：Windows 與 macOS；Linux/WSL 保留現有 Chrome 支援，但必須另外驗證企業登入政策。

## 6. CLI 契約

### 6.1 Provider 名稱

新增：

```text
--provider m365
```

設定檔接受以下輸入並統一序列化為 `m365`：

- `m365`
- `m365-copilot`
- `m365_copilot`
- `microsoft365`
- `microsoft-365-copilot`

不建議使用單獨的 `copilot`，避免與 GitHub Copilot、Windows Copilot 或一般 Microsoft Copilot 混淆。

### 6.2 V2 實際功能矩陣

| 功能 | Runtime 狀態 | 已完成實作／實站證據 | 開放前缺口 |
|---|---|---|---|
| 純文字 prompt、`login`、`open`、`get`、`--new`、`--output`、`--timeout` | experimental 支援 | 沿用 V1 已驗證流程 | 持續 smoke test |
| `--session-url`／URL 型 `--session` | Windows-only experimental | URL-only 契約、ownership、Chrome restart／resume 與 Thread Link 一致性已通過 | macOS 待後續驗證 |
| `--session-id` | 不支援 | V2 採 URL-only；不由 raw ID 重建 URL | 只有跨租戶證明 ID 足夠時才重新評估 |
| `--model` | Windows-only experimental | en-US／zh-TW、nested picker、同列副標題與 selected-state 驗證已通過 | macOS、其他租戶待驗 |
| `--reasoning` | Windows-only experimental | `Auto`／`Quick response`／`Think deeper` 與 zh-TW aliases、selection kind、衝突拒絕已通過 | macOS、其他租戶待驗 |
| `--file` | Windows-only experimental | PDF、DOCX、TXT 保證支援；其他格式不由 CLI 預先封鎖，依 `accept` 與租戶政策判定 | DLP 未實站驗證，列已知限制 |
| `--image` | Windows-only experimental | PNG、JPEG、preview、順序、混合附件、remove 與 retry 已通過 | DLP 未實站驗證，列已知限制 |
| `--image-output` | Windows-only experimental | data URL PNG 寫檔、最新 assistant scope、bytes 判型、無圖片非零錯誤已通過 | HTTP/blob、DLP 與 macOS 待驗 |

M365 V2 的 runtime capability 僅在 Windows 開啟；macOS／Linux 對 V2 旗標 browser-before fail-fast。純文字 M365 provider 維持跨平台。DLP 未完成專用租戶驗證，屬 Windows experimental 已知限制；明確 policy denial 仍禁止任何 fallback 繞過。

### 6.3 錯誤訊息

非 Windows 使用 V2 功能時應在啟動 Chrome 前失敗，例如：

```text
Error: Microsoft 365 Copilot V2 --model is Windows-only experimental in this ask-bridge version.
```

登入失敗應提示：

```text
Error: You are not logged in to Microsoft 365 Copilot.
Run `ask-bridge --provider m365 login` and complete Microsoft Entra sign-in manually.
```

若租戶政策阻擋 headless 或需要重新驗證，應提示使用 headful 模式重新登入，不得自動繞過政策。

## 7. 架構設計

### 7.1 Provider variant

在 `src/main.rs` 新增：

```rust
enum Provider {
    ChatGpt,
    Gemini,
    Claude,
    M365Copilot,
}
```

需同步更新：

- `Provider::from_config_value`
- `Provider::display_name`
- `Provider::home_url`
- `Provider::from_url`
- `Provider::conversation_url_from_id`
- `Provider::owns_conversation_url`
- `Provider::ready_check_js`
- `Provider::login_signals_js`
- `Provider::assistant_selector`
- `Provider::latest_response_selector`
- `Provider::response_content_selector`
- `Provider::composer_selectors_json`
- `Provider::send_button_selectors_json`
- `Provider::stop_button_selectors_json`
- `fmt::Display`
- 所有 exhaustive `match`

### 7.2 Provider capability matrix

新增集中式 capability，避免功能支援持續散落於條件判斷：

```rust
struct ProviderCapabilities {
    session: SessionSupport,
    images: bool,
    files: bool,
    model_selection: bool,
    reasoning: bool,
    image_download: bool,
}
```

由 `Provider::capabilities()` 回傳能力，並由 `validate_provider_feature_support` 統一驗證。

M365 V2 平台設定：

```text
Windows: session=UrlOnly, images/files/model_selection/reasoning/image_download=true
macOS/Linux: session=None, images/files/model_selection/reasoning/image_download=false
```

既有 provider 能力必須依目前行為填入，不得因重構而改變。

### 7.3 URL ownership

`Provider::from_url` 至少接受：

- `m365.cloud.microsoft`
- `www.m365.cloud.microsoft`
- `m365copilot.com`
- `www.m365copilot.com`

不得將以下登入或共用網域視為 conversation URL：

- `login.microsoftonline.com`
- `office.com`
- `www.office.com`
- 任意租戶自訂登入網域

登入重新導向仍由目前被選取的 page 持續追蹤，不透過 `Provider::from_url` 推論。

### 7.4 登入狀態

M365 login signals 必須回傳現有 `LoginSignals`：

```text
account
auth_control
auth_path
composer
stable
```

建議判斷：

- `auth_path=true`：
  - host 為 `login.microsoftonline.com`
  - URL path 或畫面明確位於登入流程
- `account=true`：
  - 可見的帳號、個人資料、Microsoft 365 account manager 或使用者選單
- `auth_control=true`：
  - 可見的 Sign in／登入控制項
- `composer=true`：
  - 可見且可輸入的 Copilot Chat composer
- `stable=true`：
  - DOM 已完成初始 hydration，訊號在短時間內保持一致

不得僅以 composer 存在判定登入成功，除非實站證明匿名使用者不可能取得可互動 composer，且有測試覆蓋。

### 7.5 DOM selector 策略

selector 優先順序：

1. 穩定的 `data-testid` 或產品專用 attribute。
2. ARIA role 與 aria-label。
3. 語意 HTML 結構。
4. 穩定 class name。
5. 可見文字，僅作最後 fallback 並同時支援英文與繁體中文。

禁止依賴：

- 動態產生的 CSS module hash。
- 單一深層 CSS hierarchy。
- 僅適用單一語系的按鈕文字。
- DOM index，例如固定取第三個按鈕。

需要校準的 selector：

- Composer
- Send button
- Stop generating button
- Assistant response container
- Response content root
- Account menu
- Sign-in control
- Copy response button

### 7.6 Prompt 提交

優先沿用 `submit_regular_prompt`：

1. 找到 composer。
2. 聚焦並清除既有內容。
3. 依序嘗試 paste event、`execCommand("insertText")` 與 value／innerText fallback。
4. 等待 send button 可見且 enabled。
5. 點擊送出。

若 M365 UI 使用 iframe 或 shadow root，必須先確認 `chrome-devtools-mcp` 是否能直接操作；若不能，應新增 provider-specific submit path，而不是擴大通用 selector 造成其他 provider 回歸。

### 7.7 回覆完成偵測

第一版沿用：

- 送出前記錄 assistant message 數量。
- 回覆期間偵測 stop button。
- assistant message 數量增加且 stop button 消失後，連續三次判定完成。

實站探勘時必須確認：

- 串流開始前是否先建立空 response container。
- response container 是否在生成過程中被替換。
- stop button 是否在所有租戶與語系出現。
- 回覆結束後是否仍存在停止或取消按鈕。

若 assistant container 會先建立空節點，完成條件需增加文字穩定檢查：

```text
最後一則回覆文字非空，且連續 N 次輪詢內容不再改變。
```

### 7.8 回覆擷取

擷取順序：

1. 優先點擊 M365 回覆工具列的 Copy 按鈕。
2. clipboard 不可用、按鈕不存在或內容未更新時，使用 DOM Markdown scraper。
3. DOM scraper 需忽略按鈕、隱藏文字、工具列及來源卡片的重複內容。

第一版至少保留：

- 標題
- 段落
- 粗體與斜體
- 清單
- inline code 與 code block
- 超連結
- Copilot 引用連結

若 Adaptive Card 或 citation 元素無法可靠轉換，第一版可降級為可讀純文字，但不得輸出空內容後仍回傳成功。

## 8. 實作階段

### Phase 0：登入後 DOM 探勘

預估：0.5–1.5 個工作天。

工作：

- 以 ask-bridge 專用 Chrome profile 完成 Entra 登入。
- 記錄登入前、登入中、登入後與 MFA 畫面的 URL 和可見語意元素。
- 送出至少三種 prompt：
  - 短文字回覆
  - 含 Markdown 清單與連結的回覆
  - 長時間串流回覆
- 觀察 response container、stop button、copy button 與 conversation URL。
- 分別記錄英文及繁體中文 UI selector。
- 確認 headful 登入後，下一次 headless Chrome 是否能沿用登入狀態。

退出條件：

- Composer、send、assistant、stop 或文字穩定條件均有可靠 selector。
- 可明確區分 LoggedIn、LoggedOut 與 Unknown。
- 確認回覆擷取至少能取得完整純文字。
- 若以上任一項不可達成，停止 Phase 1 並重新評估 API 路徑。

### Phase 1：Experimental 純文字 provider

預估：2–4 個工作天。

工作：

- 新增 `Provider::M365Copilot`。
- 新增 CLI/config aliases。
- 實作 URL ownership 與 home URL。
- 實作 ready check、login signals 與核心 DOM selectors。
- 接入純文字 prompt 提交與回覆完成偵測。
- 接入 copy/DOM 擷取。
- 新增 capability matrix 並拒絕未支援參數。
- 確保 `--new` 不重用既有 M365 分頁。
- 輸出目前頁面的 Thread Link；若網址不是可重用 conversation URL，文件需標註。

退出條件：

- 同一登入 profile 連續執行十次短 prompt，至少九次成功。
- 一次長回覆在 timeout 內完整擷取。
- 未登入狀態能在送出 prompt 前安全中止。
- ChatGPT、Gemini、Claude 測試全部維持通過。

### Phase 2：穩定化與基本發布

預估：3–5 個工作天。

工作：

- 驗證英文與繁體中文 UI。
- 驗證 Windows 與 macOS。
- 處理登入 token 過期、MFA 重新驗證與 Conditional Access 錯誤提示。
- 對 selector 加入多層 fallback，但維持語意優先。
- 增加空回覆、Copilot 服務忙碌、內容政策拒絕與 timeout 處理。
- 更新 README、quick start、CHANGELOG、package metadata 與網站文案。
- 將 provider 文件標示為 experimental，列出已知限制。

退出條件：

- 文字提問主流程具有明確成功與失敗狀態。
- 不得以空字串作為成功回覆。
- 不得在已知 LoggedOut 狀態嘗試送出。
- 文件中的命令與實際 CLI help 一致。

### Phase 3：選配功能逐項解鎖

每項功能必須獨立驗證與發布，不得一次假設通用機制皆可使用。

#### 3.1 Session resume

- 取得並記錄真實 conversation URL 格式。
- 驗證 URL 在重新開啟後仍可存取同一對話。
- 只有 ID 可穩定轉換成 URL 時才開放 raw `--session-id`。
- 若 URL 包含租戶或一次性狀態，只支援完整 `--session-url`。

#### 3.2 File attachment

- 優先使用可存取的 upload button 與 `upload_file`。
- 驗證檔名 chip、上傳完成與失敗狀態。
- 只保證支援 PDF／DOCX／TXT；其他格式依 M365 當下的 `accept` 規則與租戶政策嘗試上傳，不由 CLI 預先封鎖。

#### 3.3 Image attachment

- 驗證 hidden file input、DataTransfer 或 file chooser。
- 確認企業 DLP 政策拒絕時能回報實際錯誤。

#### 3.4 Model or mode selection

- 僅在 UI 明確提供使用者可切換且能驗證選取狀態時實作。
- 不得將 Microsoft 自動選模推論成可用的 `--model`。
- 若實作，擴充 `src/model-selection.cjs` 與對應 Node tests。

### Phase 3 Windows／en-US inventory（2026-08-11）

- Conversation URL：`https://m365.cloud.microsoft/chat/conversation/<id>`；ID 為單一路徑 segment。V2 採 URL-only，query／fragment、額外 path 與 `m365copilot.com` conversation URL 均 fail-closed。
- URL 重開：在已登入 profile 對 conversation URL 加入任意 query 或 fragment 仍會載入同一安全測試對話；V2 ownership 仍保守拒絕未經跨租戶驗證的 query／fragment。將相同 path 改用 `m365copilot.com` 會回到 `m365.cloud.microsoft/chat` home，不可視為 conversation alias。
- Picker：新對話頁的可見 `#gptModeSwitcher`／`aria-label="Model Selector"`。既有 conversation 只保留隱藏 picker，因此切換時需提示使用 `--new`。
- 切換後狀態：新對話頁由 `Auto` 切至 `Quick response` 時，既有 composer 草稿與已完成的 `sample.txt` attachment chip 均保留；既有 conversation 則無可見 picker。
- Reasoning 主選項：en-US `Auto`、`Quick response`、`Think deeper`；zh-TW `自動`、`快速回應`、`深度思考`。
- Nested model 主選項：`GPT 5.6`、`GPT 5.5`、`Sonnet`、`Opus`。副標題例如 `Think deeper`、`Quick response`、`快速回應`、`OpenAI`、`Anthropic` 不屬於 model 名稱。
- 當前帳號的上述選項均未呈現 `aria-disabled`、locked、premium 或 policy-blocked state；helper 仍需對 disabled／locked fail-closed，不能推論其他租戶相同。
- Upload：`[data-testid="PlusMenuButton"]` → en-US `Upload images and files`／zh-TW `上傳影像和檔案`，hidden `input[type=file][multiple]`；文件只保證 PDF／DOCX／TXT，其他格式依當下 `accept` 與租戶政策嘗試，圖片維持 PNG／JPEG。
- zh-TW 附件：Unicode 檔名 chip 保持原名，remove button 為 `移除附件 <filename>`。
- 附件完成訊號：composer scope 出現檔名 chip 與 `Remove attachment <filename>`；圖片另有檔名與 preview。
- 多附件：兩張 PNG 依指定順序顯示；圖片＋文件混合可同時完成。Unicode／長檔名可顯示；兩個不同路徑但同名的檔案只呈現單一 chip，因此 V2 不宣稱同名檔可獨立識別。
- 已觀察下限而非上限：5 MB TXT 可完成；兩張 PNG 與圖片＋文件可並存；4096×4096 PNG 會產生 2048×2048 preview。未觀察到可靠的最大檔案大小或最大附件數，因此文件不得宣稱固定上限。
- 拒絕案例：不支援副檔名顯示 `This file type is not supported`；零位元 PDF 顯示 `Upload failed - file is empty`；損毀 PNG 未產生 chip 或可見 alert；損毀 DOCX 仍可能先出現完成 chip，因此 CLI 必須保留本機 signature preflight，不能只相信 UI chip。
- 上傳進度：5 MB TXT 的 chip 會顯示 `upload in progress, 40%`，但 Send 仍可用；CLI 必須自行等待 chip 不再呈現 pending／percentage，不能依賴按鈕 disabled state。
- 圖片 preview：4096×4096 PNG 先出現 `upload in progress` 且尚無 preview，之後才出現 2048×2048 preview；Send 全程仍可用，因此 CLI 必須等待 preview／attachment identity 完成。
- 部分失敗與 retry：同批指定 `sample.txt` 與不支援的 `.exe` 時，TXT chip 成功但 UI 同時顯示 file type error；點擊 `Upload a different file` 後可成功重試 PNG。CLI 必須保留逐檔結果，任一失敗不得回報全部成功。
- 生成圖片：最新 assistant turn 中 `alt="Generated image"` 的大圖；本環境來源為 data URL，並出現 `Download` control。
- Windows CLI E2E：
  - `--session-url` 成功在原 conversation 送出後續 prompt，輸出的 Thread Link 與要求 URL 相同。
  - `--model "GPT 5.5"` 與 `--reasoning quick` 均通過最終 selected-state 驗證後送出 prompt。
  - `--file sample.txt` 與 `--image bridge-emblem.png` 均透過 Rust → MCP `upload_file` → M365 完成並取得回覆。
  - `get <generated-image-conversation> --image-output <dir>` 寫出實際 PNG；純圖片回覆允許略過空 Markdown 後繼續掃描。
  - 對無生成圖片的 conversation 指定 `--image-output` 會回傳非零 `no generated image` 錯誤。
- E2E 修正：picker／upload control 需等待 hydration；M365 選單的主標籤與副標題可能在同一行，必須依已觀察主標籤拆分；純圖片回覆不得在 image scan 前因空文字中止。
- 未驗證：zh-TW 完整 CLI E2E、macOS、第二帳號／租戶、MFA／Conditional Access resume、DLP blocked、HTTP/blob generated image。

### Windows V2 smoke commands

| 能力 | Smoke command | 2026-08-11 結果 |
|---|---|---|
| Session URL | `ask-bridge --provider m365 --session-url "<conversation-url>" "Reply with exactly: SESSION RESUMED"` | 通過；Thread Link 一致 |
| Model | `ask-bridge --provider m365 --new --model "GPT 5.5" "Reply with exactly: MODEL SELECTED"` | 通過 |
| Reasoning | `ask-bridge --provider m365 --new --reasoning quick "Reply with exactly: REASONING SELECTED"` | 通過 |
| File | `ask-bridge --provider m365 --new --file sample.txt "Reply with exactly: FILE ATTACHED"` | 通過 |
| Image | `ask-bridge --provider m365 --new --image sample.png "Describe this image"` | 通過 |
| Image download | `ask-bridge --provider m365 --image-output output get "<generated-image-conversation-url>"` | 通過；PNG signature 正確 |

上述命令只列無敏感測試資料；DLP blocked 未實站驗證。macOS 驗證流程見 `docs/quick-start.md`。

## 9. 檔案變更清單

### 必要

| 檔案 | 預期變更 |
|---|---|
| `src/main.rs` | Provider variant、capability、URL、登入、selector、驗證與 tests |
| `README.md` | 繁中功能、安裝、登入、限制與範例 |
| `README.en.md` | 英文對應文件 |
| `docs/quick-start.md` | M365 login/config/query 指令 |
| `CHANGELOG.md` | 新 provider 與已知限制 |
| `package.json` | description、keywords |
| `PRODUCT.md` | 支援 provider 與產品定位 |

### 視功能需要

| 檔案 | 觸發條件 |
|---|---|
| `src/model-selection.cjs` | 開放 M365 模型或模式選擇 |
| `tests/model-selection.test.cjs` | 修改 model selection helper |
| `scripts/ask.sh` | 該 legacy wrapper 仍屬正式支援面 |
| `public/index.html`、`public/app.js` | 官方網站需顯示 M365 支援 |
| `skills/ask-bridge/SKILL.md` | Agent Skill 應能選擇 M365 provider |

`mcp_servers.json` 原則上不需修改，M365 UI 仍使用相同 Chrome DevTools MCP server。

## 10. 測試計劃

### 10.1 Rust 單元測試

新增或更新：

- CLI 可解析 `--provider m365`。
- config aliases 可解析並序列化為 `m365`。
- provider precedence 不變。
- `Provider::from_url` 僅接受允許的 M365 host 與 HTTPS。
- 登入網域不得被當作 M365 conversation URL。
- M365 未支援參數會被明確拒絕。
- capability matrix 保留既有 provider 行為。
- M365 selector baseline 非空。
- M365 login signal script 包含必要的 Entra 與 account/composer 訊號。
- URL 推論與 provider 衝突檢查不影響既有 provider。

### 10.2 JavaScript tests

只有在修改 `src/model-selection.cjs` 時新增：

- M365 picker 偵測。
- 主標籤比對。
- selected state 驗證。
- 找不到選項時列出可用選項。

### 10.3 手動端到端矩陣

| 案例 | Windows | macOS |
|---|---:|---:|
| 首次 Entra 登入 | 必測 | 必測 |
| MFA 登入 | 必測 | 擇一 |
| 已登入 headless query | 必測 | 必測 |
| `--new` 新對話 | 必測 | 必測 |
| 短回覆 | 必測 | 必測 |
| 長串流回覆 | 必測 | 必測 |
| 含清單、code block、連結 | 必測 | 必測 |
| 登入逾時 | 必測 | 擇一 |
| 服務忙碌／錯誤訊息 | 擇一 | 擇一 |
| zh-TW UI | 必測 | 擇一 |
| en-US UI | 必測 | 擇一 |

### 10.4 必跑命令

```powershell
cargo fmt --all -- --check
cargo test
cargo check
npm test
```

若 Cargo target 目錄空間不足，可沿用專案既有慣例將 `CARGO_TARGET_DIR` 指向 `%TEMP%`。

## 11. 驗收條件

### 功能

- `ask-bridge --provider m365 login` 可完成手動登入並偵測登入成功。
- 已登入後可在 headless 模式送出純文字 prompt。
- 工具可辨識新回覆、等待生成完成並輸出非空內容。
- `--output` 內容與終端主要回覆一致。
- `--new` 不得覆寫或誤用既有 provider 分頁。
- 未支援功能在操作瀏覽器前回報明確錯誤。

### 相容性

- ChatGPT、Gemini、Claude 的 CLI、登入、附件與模型行為不變。
- 全域 config 仍預設為 ChatGPT。
- M365 URL 不得誤判成其他 provider。
- 非 M365 Microsoft 網址不得被當成可恢復的 session。

### 安全與隱私

- 不記錄帳號、token、cookie、prompt 或回覆內容至一般 log。
- verbose log 不得輸出頁面 HTML、access token 或登入表單內容。
- 使用既有專用 Chrome profile，不讀取使用者日常 Chrome profile。
- 不繞過 Microsoft Entra、MFA、Conditional Access、DLP 或租戶政策。
- 文件需提醒使用者：企業資料的使用仍受其組織政策與 Microsoft 365 權限控管。

## 12. 風險與緩解

| 風險 | 等級 | 緩解方式 |
|---|---:|---|
| M365 DOM 頻繁變更 | 高 | 語意 selector、多 fallback、experimental 標記、實站 smoke test |
| Entra MFA／Conditional Access 阻擋 headless | 高 | login 強制 headful、清楚提示重新驗證、不嘗試繞過 |
| 不同租戶 UI 或 rollout 差異 | 高 | 至少兩個租戶或帳號驗證；selector 不依賴單一 class |
| 回覆節點先建立後串流 | 中 | stop button 加文字穩定檢查 |
| Copy 按鈕或 clipboard 不可靠 | 中 | DOM Markdown scraper fallback |
| conversation URL 不穩定 | 中 | 第一版停用 `--session` |
| 企業資料誤輸出到終端或檔案 | 中 | 明確文件、沿用使用者權限、不增加自動持久化 |
| 新 variant 造成 exhaustive match 回歸 | 中 | capability matrix、單元測試、完整 Rust tests |

## 13. Rollout

1. 先在 feature branch 完成 Phase 0 與 Phase 1。
2. 文件標示 `m365` 為 experimental。
3. 由至少兩個不同 Microsoft 365 帳號執行 smoke test。
4. 發布後監控：
   - composer not found
   - send button disabled timeout
   - no assistant message found
   - response timeout
   - login state unknown
5. selector 失效時應快速停用 M365 provider 或回報明確錯誤，不得影響其他 provider。

## 14. 官方 Chat API 後續路徑

Microsoft 官方已提供 Microsoft 365 Copilot Chat API Preview：

- 建立對話：`POST https://graph.microsoft.com/beta/copilot/conversations`
- 串流對話：`POST https://graph.microsoft.com/beta/copilot/conversations/{conversationId}/chatOverStream`

此路徑不納入第一版，原因：

- `/beta` API 不支援正式 production SLA。
- 只支援具有 Microsoft 365 Copilot add-on 的工作或學校帳號。
- 需要 Microsoft Graph delegated permissions，且不支援 application permission。
- 現有專案尚無 OAuth、token cache、HTTP client 與 SSE parser。

若未來實作，應先重構：

```text
Provider
  -> BrowserProvider transport
  -> GraphCopilot transport
```

API transport 至少需要：

- Entra app registration。
- Device code 或 localhost redirect OAuth。
- 安全 token cache。
- 租戶與 consent 錯誤處理。
- Graph throttling 與 retry-after。
- SSE parser。
- conversation ID 保存與恢復。
- citation、adaptive card 與 Markdown 轉換。

預估：

- API 技術 POC：1–2 週。
- 可供一般企業使用的 authentication、consent 與錯誤處理：再增加 2–3 週。

## 15. 參考資料

- [Microsoft 365 Copilot Chat overview](https://learn.microsoft.com/en-us/copilot/overview)
- [Minimum requirements for Microsoft 365 Copilot Chat](https://learn.microsoft.com/en-us/microsoft-365/copilot/microsoft-365-copilot-chat-requirements)
- [Microsoft 365 Copilot Chat API Overview](https://learn.microsoft.com/en-us/microsoft-365/copilot/extensibility/api/ai-services/chat/overview)
- [Create a Copilot conversation](https://learn.microsoft.com/en-us/microsoft-365/copilot/extensibility/api/ai-services/chat/copilotroot-post-conversations)
- [Stream a Copilot conversation](https://learn.microsoft.com/en-us/microsoft-365/copilot/extensibility/api/ai-services/chat/copilotconversation-chatoverstream)
- [Microsoft 365 Copilot APIs Terms of Use](https://learn.microsoft.com/en-us/legal/m365-copilot-apis/terms-of-use)
