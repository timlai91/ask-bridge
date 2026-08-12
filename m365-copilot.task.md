# Microsoft 365 Copilot Provider 實作工作計劃

> 依據：`m365-copilot.spec.md`  
> 狀態：核心 Phase 1 experimental 實作完成；跨平台、完整回歸與錯誤分類待補
> 第一版定位：Experimental、Chrome UI 自動化、僅支援純文字提問與文字回覆  
> 建議 provider 名稱：`m365`  
> 預估工期：Phase 0–2 共 5.5–10.5 個工作天；Phase 3 選配功能另計

## 1. 目標與完成定義

- [ ] `ask-bridge --provider m365 login` 可用專屬 Chrome profile 完成 Microsoft Entra 手動登入、MFA 與登入完成偵測。
- [x] 已登入後可用 headless Chrome 執行純文字 prompt，等待完整回覆並輸出非空 Markdown 或可讀純文字。
- [x] `ask-bridge config --provider m365` 可保存設定，所有規格別名均統一序列化為 `m365`。
- [x] `ask-bridge --provider m365 --new "<prompt>"` 會建立新的 M365 Copilot Chat 分頁或新對話，不誤用其他 provider 或既有 M365 分頁。
- [x] `--output`、`--timeout`、`login`、`open` 與實驗性的 `get` 行為符合 CLI 契約。
- [x] `--session`、`--model`、`--reasoning`、`--image`、`--file` 與圖片下載在第一版啟動 Chrome 前即回報明確錯誤。
- [ ] ChatGPT、Gemini、Claude 的既有 CLI、登入、附件、模型、推理、session 與圖片下載行為不變。
- [x] README、quick start、產品描述、套件 metadata、網站與 Agent Skill 的支援範圍與實際 CLI 一致。
- [ ] Windows 與 macOS 的必要手動矩陣完成，且 M365 provider 清楚標示為 experimental。

## 2. 範圍界線

### 2.1 第一版包含

- Microsoft 365 Copilot Chat 網頁 UI 自動化。
- Microsoft Entra 工作或學校帳號人工登入。
- MFA、登入逾時、登入失效與 Conditional Access 提示。
- 純文字 prompt 提交。
- 回覆完成偵測。
- Copy 按鈕優先、DOM Markdown scraper fallback 的回覆擷取。
- 英文與繁體中文 UI selector。
- Windows 與 macOS 支援。

### 2.2 第一版不包含

- Microsoft Graph／Copilot Chat API、OAuth、token cache 或 SSE。
- 模型、模式、推理強度或 Agent 選擇。
- Session ID／conversation URL 恢復。
- 圖片或文件附件。
- 圖片產生與下載。
- 郵件、Teams、SharePoint、會議、檔案建立等企業 action。
- 繞過 MFA、CAPTCHA、Conditional Access、DLP 或租戶政策。
- 所有租戶、國家雲或 rollout 變體的相容性保證。

## 3. 前置條件與硬性閘門

- [x] 準備至少一個具 Microsoft 365 Copilot Chat 使用資格的工作或學校帳號。
- [ ] 確認可由測試人員完成 MFA。
- [x] 準備 en-US 與 zh-TW UI；若只能取得一種語系，記錄未驗證語系並限制第一版聲明。
- [ ] 優先準備兩個不同帳號或租戶，以降低單一 rollout 造成的 selector 偏差。
- [x] 確認 `https://m365.cloud.microsoft/chat` 與 `https://m365copilot.com` 的實際重新導向行為。
- [x] 確認測試過程不保存或提交 token、cookie、帳號、prompt、回覆、登入表單 HTML 等敏感資料。

**硬性閘門：** Phase 0 必須先證明登入狀態、composer、send、assistant response、停止生成或文字穩定條件，以及完整純文字擷取均可可靠辨識。任一必要訊號不可達成時，不進入 Phase 1，改評估 Microsoft 365 Copilot Chat API transport。

## 4. 里程碑總覽

| 階段 | 預估 | 主要產出 | 退出條件 |
|---|---:|---|---|
| Phase 0：登入後 DOM 探勘 | 0.5–1.5 天 | selector 與登入狀態基線、回覆生命週期紀錄 | 必要 DOM 訊號可靠且可取得完整純文字 |
| Phase 1A：Provider 與 CLI 契約 | 0.5–1 天 | `M365Copilot`、aliases、capability matrix、fail-fast | CLI/config/未支援功能測試通過 |
| Phase 1B：頁面、登入與提交 | 1–1.5 天 | URL ownership、登入偵測、selector、prompt submit | 登入前安全中止，登入後可送出 prompt |
| Phase 1C：完成判定與回覆擷取 | 0.5–1.5 天 | completion polling、Copy/DOM fallback、非空保證 | 短答與長串流皆可完整擷取 |
| Phase 1D：回歸與 smoke test | 0.5–1 天 | Rust tests、10 次連續查詢、既有 provider 回歸 | 10 次短查詢至少 9 次成功 |
| Phase 2：穩定化與文件發布 | 3–5 天 | 雙語系、跨平台、錯誤狀態、文件與網站 | 功能、錯誤、安全與文件驗收完成 |
| Phase 3：選配功能 | 另計 | session、附件、模式選擇逐項發布 | 每項功能獨立驗證後才解鎖 |

## 5. Phase 0：登入後 DOM 探勘

### 5.1 建立安全的探勘環境

- [x] 使用 ask-bridge 專屬 Chrome profile，不讀取日常 Chrome profile。
- [x] 以 headful Chrome 開啟 `https://m365.cloud.microsoft/chat`。
- [ ] 記錄登入前、帳號選擇、密碼、MFA、Conditional Access、登入完成等階段的：
  - host 與 URL path 類型。
  - 可見 ARIA role、aria-label、`data-testid` 或產品 attribute。
  - account menu、Sign in、composer 等語意元素。
- [x] 僅記錄 selector 與狀態摘要；不得提交完整 HTML、token、cookie、帳號或企業內容。

### 5.2 建立 selector inventory

- [x] 依下列優先順序為每個元素建立 primary 與 fallback：
  1. 穩定 `data-testid` 或產品專用 attribute。
  2. ARIA role 與 aria-label。
  3. 語意 HTML 結構。
  4. 經確認穩定的 class。
  5. 同時涵蓋 en-US／zh-TW 的可見文字。
- [x] 校準 Composer selector。
- [x] 校準 Send button selector與 disabled 狀態。
- [x] 校準 Stop／Cancel generation selector。
- [x] 校準 assistant response container。
- [x] 校準 response content root。
- [x] 校準 account menu。
- [x] 校準 Sign in／登入控制項。
- [x] 校準 Copy response button。
- [x] 確認 selector 不依賴動態 hash、深層 hierarchy、單一語系文字或固定 DOM index。

### 5.3 驗證登入狀態模型

- [x] 驗證 `login.microsoftonline.com` 與其他 Entra 登入 path 可穩定判定 `auth_path=true`。
- [x] 驗證 account menu 可穩定判定 `account=true`。
- [x] 驗證可見 Sign in／登入按鈕可判定 `auth_control=true`。
- [x] 驗證可輸入 composer 可判定 `composer=true`，但不得單獨作為 M365 已登入依據。
- [x] 測量 SPA hydration 後訊號穩定所需時間，定義 `stable=true` 的輪詢條件。
- [x] 分別保存 LoggedIn、LoggedOut、Unknown 的可重現判斷依據。
- [x] 確認 headful 登入完成後，下一次 headless Chrome 可沿用登入狀態。

### 5.4 驗證 prompt 與回覆生命週期

- [x] 送出短文字 prompt。
- [x] 送出含標題、清單、code block 與連結的 prompt。
- [x] 送出長時間串流 prompt。
- [x] 記錄送出前後 assistant message 數量變化。
- [x] 確認 response container 是延遲建立、先建立空節點，或生成過程中會被替換。
- [ ] 確認 stop button 是否在不同語系與帳號 rollout 中出現。
- [x] 確認生成結束後 stop／cancel 元素是否仍殘留。
- [x] 測量「最後回覆文字非空且連續 N 次不再變化」所需的合理 N 值與輪詢間隔。
- [ ] 驗證 Copy 按鈕是否寫入完整 Markdown，以及 clipboard 在 Windows／macOS 的差異。
- [x] 驗證 DOM scraper 至少能取得完整純文字。
- [x] 記錄當前 conversation URL，判斷是否為可重開、租戶相依或一次性 URL；第一版仍不開放 session。

### 5.5 Phase 0 退出審查

- [x] Composer、send、assistant、copy 與完成判定均有至少一個可靠 primary selector。
- [x] Stop button 或文字穩定條件至少一者可靠，且另一者可作 fallback。
- [x] LoggedIn、LoggedOut 與 Unknown 可明確區分。
- [x] 未登入時不會出現可誤送 prompt 的成功判斷。
- [x] Copy 或 DOM fallback 可輸出完整且非空的回覆。
- [x] iframe／shadow root 使用情況已確認。
- [x] 若 chrome-devtools-mcp 無法操作必要 iframe／shadow root，已決定 provider-specific submit／scrape path，而非擴大通用 selector。

### 5.6 執行紀錄

#### 2026-08-10：登入前基線

- 使用 `C:\Users\Tim\.config\ask-bridge\chrome-profile` 建立專屬 persistent Chrome profile，未讀取日常 Chrome profile。
- 以 headful Chrome 開啟兩個入口；`m365.cloud.microsoft/chat` 與 `m365copilot.com` 均重新導向 `login.microsoftonline.com/common/oauth2/v2.0/authorize`，redirect target 為 M365 landing page。
- en-US 登入頁可見 email input、password input、Sign in submit、建立帳號與帳號存取協助等語意控制項。
- 初始 Entra 登入頁未發現 iframe 或 open shadow root；此結果只適用登入前頁面，不代表 Copilot Chat 頁面。
- 未保存或提交帳號、輸入值、token、cookie、OAuth nonce/state、完整 URL 或登入頁 HTML。
- 使用者本次未完成 Entra 登入與 MFA；登入後 composer、account menu、response selectors、prompt、完成判定、Copy 與 DOM fallback 尚未驗證。
- 隨後改以 headed Chrome 完成 Entra 登入；本次未確認是否實際觸發 MFA，因此 MFA 項目維持未勾選。
- Windows／en-US 已驗證登入後與 headless 延續；zh-TW、第二帳號／租戶與 macOS 留待 Phase 2。
- **狀態：Phase 0 必要硬性閘門已通過，可開始 Phase 1；未驗證矩陣維持未勾選。**

#### 2026-08-10：登入後 selector inventory

| 元素 | Primary | Fallback／補充 |
|---|---|---|
| Composer | `#m365-chat-editor-target-element` | `[role="textbox"][contenteditable="true"][aria-label*="Copilot"]` |
| Composer scope | `#m365-chat-input-shared-container` | 由 composer 向上限縮至輸入容器 |
| Send | `#m365-chat-input-shared-container button[type="submit"][aria-label="Send"]` | 輸入容器內 enabled `button[type="submit"]` |
| Stop | `#m365-chat-input-shared-container button[type="submit"][aria-label="Stop generating"]` | 文字穩定條件作跨語系 fallback |
| Assistant turn | `[data-testid="copilot-message-div"]` | `[data-testid="m365-chat-llm-web-ui-chat-message"] [data-testid="chatOutput"]` |
| Latest response | `[data-testid="copilot-message-div"]` 最後一個 | 內部 `[data-testid="lastChatMessage"]` |
| Response content | `[data-testid="markdown-reply"]` | `[data-testid="lastChatMessage"]` 或 assistant turn |
| Account marker | `#user-account-avatar` | M365 host + composer + 無 auth control 只作 Unknown／輔助訊號 |
| Sign-in control | `input[name="loginfmt"]`、`#idSIButton9` | `login.microsoftonline.com` auth path 優先 |
| Copy response | `[data-testid="CopyButtonTestId"]` | assistant turn 內 `button[aria-label="Copy Response"]` |
| Citation | `button[data-grouped-citations]` | 解析 JSON 中的 `url` 與 `name`；點擊會開啟來源新分頁 |
| Code block | response content 內 `[role="textbox"][aria-label="Code editor"]` | 以可讀純文字 code fence 降級 |

- 回覆 turn 在送出後立即建立空節點，串流期間內容曾由非空暫時回到空字串，因此完成判定必須同時要求 stop 消失、內容非空與連續穩定。
- en-US 實測 `Stop generating` 在生成結束後消失；以 250ms 輪詢連續三次文字不變可穩定判定完成。
- Copy 按鈕在 Windows 會將完整短回覆寫入系統剪貼簿；DOM content root 同時可取得一致非空純文字。
- 對話 URL 形狀為 `/chat/conversation/<id>`，同一登入 profile 可重新開啟並載入原對話；第一版仍依規格停用 session。
- M365 Chat 主要互動 DOM 位於 top-level document；觀察到的 Microsoft login iframe 不承載 composer／response，且未發現 open shadow root。

## 6. Phase 1A：Provider 與 CLI 契約

### 6.1 新增 Provider variant

主要檔案：`src/main.rs`

- [x] 新增 `Provider::M365Copilot`，Clap 名稱為 `m365`。
- [x] `Provider::from_config_value` 接受：
  - `m365`
  - `m365-copilot`
  - `m365_copilot`
  - `microsoft365`
  - `microsoft-365-copilot`
- [x] `fmt::Display` 固定輸出 `m365`，確保 config 統一序列化。
- [x] `display_name()` 回傳 `Microsoft 365 Copilot`。
- [x] `home_url()` 回傳 `https://m365.cloud.microsoft/chat`。
- [x] 更新所有 exhaustive `match`，但不得以通用 fallback 假裝支援 M365 未驗證功能。
- [x] 更新 config 錯誤提示與 `config` help 中的 provider 列表。

### 6.2 集中 Provider capability matrix

- [x] 新增 `ProviderCapabilities`：
  - `session_id`
  - `images`
  - `files`
  - `model_selection`
  - `reasoning`
  - `image_download`
- [x] 新增 `Provider::capabilities()`。
- [x] 先以現有程式行為與測試建立 ChatGPT、Gemini、Claude 能力基線，不在重構時改變既有支援範圍。
- [x] M365 第一版所有上述能力設為 `false`。
- [x] 重構 `validate_provider_feature_support`，統一在啟動 Chrome 前檢查 capability。
- [x] 保留既有 Gemini 圖片附件專屬錯誤語意。
- [x] M365 的每個未支援參數均回傳具 provider 與參數名稱的錯誤，例如：

```text
Error: Microsoft 365 Copilot does not support --model in this ask-bridge version.
```

- [x] `--session` 必須在進入 `resolve_session_target` 前被拒絕。
- [x] 調整 `conversation_url_from_id` 的介面或呼叫順序，使不支援 session 的 provider 不必製造虛假的 conversation URL。
- [x] `--image-output` 不得掃描或下載 M365 回覆中的圖片。

### 6.3 CLI 與 config 單元測試

- [x] CLI 可解析 `--provider m365`，並支援 global option 前後位置。
- [x] 所有 config aliases 均解析成 `Provider::M365Copilot`。
- [x] config 寫回值固定為 `m365`。
- [x] CLI provider > config provider > ChatGPT 預設的 precedence 不變。
- [x] 未指定 provider 時仍預設 ChatGPT。
- [x] `copilot` 仍被拒絕，避免名稱歧義。
- [x] 每個 M365 未支援參數均有 fail-fast 測試。
- [x] capability matrix 測試鎖定三個既有 provider 的現況。

## 7. Phase 1B：URL、頁面選擇與登入

### 7.1 URL ownership

主要檔案：`src/main.rs`

- [x] `Provider::from_url` 僅接受 HTTPS。
- [x] 將下列 host 映射為 M365：
  - `m365.cloud.microsoft`
  - `www.m365.cloud.microsoft`
  - `m365copilot.com`
  - `www.m365copilot.com`
- [x] 明確排除：
  - `login.microsoftonline.com`
  - `office.com`
  - `www.office.com`
  - 任意租戶自訂登入網域
- [x] 登入重新導向期間沿用已選取 page 與原 provider，不透過登入 host 重新推論 provider。
- [x] 第一版 `owns_conversation_url` 對 M365 採 fail-closed；只有 Phase 3 驗證真實格式後才接受可恢復 URL。
- [x] 增加 URL query、fragment、相似 host、HTTP 與登入網域的負向測試。

### 7.2 分頁開啟與重用

- [x] 一般查詢只重用 M365 擁有且登入狀態可用的分頁。
- [x] 不得把 Microsoft 登入頁或其他 Microsoft 站點當作可查詢分頁。
- [x] `--new` 必須保留所有既有頁籤並建立唯一的新 M365 分頁。
- [x] 新分頁辨識不唯一時沿用既有 fail-closed 行為，停止而非猜測。
- [x] M365 分頁不得覆寫或重用 ChatGPT、Gemini、Claude 分頁。
- [x] 查詢完成後輸出目前 M365 頁面的 Thread Link。
- [x] 若 Thread Link 尚未證明可重開，輸出與文件均不得宣稱可用於 session resume。

### 7.3 Ready check 與 login signals

- [x] 新增 M365 `ready_check_js()`，接受登入 shell、登入頁或 Copilot Chat 已 hydration 的必要訊號。
- [x] 新增 M365 `login_signals_js()`，回傳完整 `LoginSignals`。
- [x] 以 account、auth control、auth path、composer 與 stable 的組合判斷登入狀態。
- [x] M365 不得套用 ChatGPT 的「只有 composer 即 LoggedIn」例外。
- [x] 登入訊號需容忍 SPA hydration，但明確 auth path 必須優先判定 LoggedOut。
- [x] `login` 強制 headful，持續追蹤同一 page 完成 Entra／MFA 流程。
- [x] 登入成功後回到 M365 Chat home，確認 account 與 composer 訊號。
- [ ] 登入逾時、token 過期、MFA 重新驗證與 Conditional Access 均提供可行動錯誤，不自動繞過政策。
- [x] 未登入查詢在 prompt 提交前回傳：

```text
Error: You are not logged in to Microsoft 365 Copilot.
Run `ask-bridge --provider m365 login` and complete Microsoft Entra sign-in manually.
```

### 7.4 登入與頁面單元測試

- [x] M365 selector baseline 皆非空。
- [x] login script 包含 Entra host／path、account、auth control、composer 與穩定化訊號。
- [x] M365 composer-only 訊號保持 Unknown。
- [x] M365 account 訊號判定 LoggedIn。
- [x] M365 auth path 或穩定 auth control 判定 LoggedOut。
- [x] 不穩定訊號不得誤判 LoggedIn／LoggedOut。
- [x] M365 host 可推論 provider，但登入 host、Office host 與相似惡意 host 不可推論。

## 8. Phase 1C：Prompt 提交、完成判定與回覆擷取

### 8.1 DOM selector 實作

- [x] 依 Phase 0 inventory 實作：
  - `assistant_selector`
  - `latest_response_selector`
  - `response_content_selector`
  - `composer_selectors_json`
  - `send_button_selectors_json`
  - `stop_button_selectors_json`
- [x] 每組 selector 先放穩定 attribute，再放 ARIA／語意 fallback，最後才放雙語文字。
- [x] selector 限縮在 M365 response／composer scope，避免命中側欄、搜尋框、來源卡片或其他 Microsoft 365 控制項。

### 8.2 Prompt 提交

- [x] 優先沿用 `submit_regular_prompt`：
  1. 找到 composer。
  2. 聚焦並清除既有內容。
  3. 依序嘗試 paste event、`execCommand("insertText")`、value／innerText fallback。
  4. 等待 send button 可見且 enabled。
  5. 點擊送出。
- [x] 驗證文字輸入後 composer 內容與原 prompt 一致。
- [x] 驗證 send button disabled timeout 可回報明確錯誤。
- [x] 若 M365 使用 iframe／shadow root，新增 M365 專屬提交路徑，不修改其他 provider 的 selector 範圍。
- [ ] 服務錯誤或登入狀態在送出前改變時安全中止。

### 8.3 回覆完成判定

- [x] 送出前記錄 assistant message 數量。
- [x] 生成期間偵測可見 stop button。
- [x] assistant message 數量增加且 stop button 消失後，連續三次確認完成。
- [x] 若 response container 會先建立空節點，加入：
  - 最後一則回覆文字非空。
  - 內容 hash／文字連續 N 次不變。
- [x] response container 被替換時，以每次輪詢重新取得最新節點，避免持有失效 DOM reference。
- [x] timeout 時不得把空字串或未完成內容當成成功。
- [ ] 服務忙碌、內容政策拒絕、一般錯誤卡片與登入失效需與正常回覆區分。

### 8.4 Copy 優先與 DOM fallback

- [x] `click_latest_copy_button` 可在最新 M365 assistant turn 的工具列內找到 Copy。
- [x] Copy selector 不得誤按 code block、table、prompt 或來源卡片的 Copy。
- [x] clipboard 未更新、不可用或按鈕不存在時，自動使用 `scrape_latest_markdown_from_dom`。
- [x] DOM scraper 至少保留：
  - 標題
  - 段落
  - 粗體與斜體
  - 清單
  - inline code 與 code block
  - 超連結
  - Copilot citation link
- [x] 忽略按鈕、隱藏文字、工具列、重複來源卡片與非回覆 UI。
- [ ] Adaptive Card 或 citation 無法可靠轉換時，降級為可讀純文字。
- [x] Copy 與 DOM fallback 的結果均必須 `trim()` 後非空。
- [x] 兩條擷取路徑均失敗或內容為空時回傳非零錯誤，不得只印 warning 後成功結束。
- [x] `--output` 寫入內容與終端主要回覆一致，不包含 Thread Link。

### 8.5 回覆流程測試

- [x] M365 assistant／latest／content selectors 非空且作用域合理。
- [x] 完成判定涵蓋 generating、waiting、空 response、穩定文字與 done。
- [x] DOM scraper 測試涵蓋標題、清單、code block、link、citation 與隱藏／工具列排除。
- [x] 空字串、只有空白、只有工具列內容均視為擷取失敗。
- [x] capability 關閉時不執行附件、model、reasoning 或圖片下載流程。
- [ ] 既有 provider 的 copy／DOM fallback 測試維持通過。

## 9. Phase 1D：Experimental 主流程驗收

### 9.1 自動測試

- [x] `cargo fmt --all -- --check`
- [x] `cargo test`
- [x] `cargo check`
- [x] `npm test`
- [x] 若 workspace target 空間不足，將 `CARGO_TARGET_DIR` 指向 `%TEMP%`。

### 9.2 M365 smoke test

- [x] 同一登入 profile 連續執行 10 次短 prompt，至少 9 次成功。
- [ ] 失敗案例需分類為 selector、登入、送出、完成判定、擷取或服務端錯誤。
- [x] 執行一次長串流回覆，確認 timeout 內完整擷取。
- [x] 執行一次含標題、清單、code block、一般連結與 citation 的回覆。
- [x] 驗證 `--output` 與終端回覆一致。
- [x] 驗證 `--new` 建立新分頁且保留既有頁籤。
- [ ] 驗證未登入、登入過期或 auth redirect 時不送出 prompt。
- [x] 驗證每個未支援參數均在 Chrome 操作前失敗。
- [x] 驗證一般查詢後會輸出目前頁面的 Thread Link。

### 9.3 既有 provider 回歸

- [ ] ChatGPT：登入判斷、純文字、session、附件、model、reasoning、圖片下載。
- [ ] Gemini：純文字、session、文件附件、model、reasoning，且圖片附件仍明確拒絕。
- [ ] Claude：純文字、session、圖片／文件附件、model，且 reasoning 仍明確拒絕。
- [x] 全域 config 預設與 provider precedence 不變。
- [ ] `open`、`get`、`close`、`dump`、`screenshot` 的既有行為不被 M365 variant 破壞。

### 9.4 2026-08-10 驗證紀錄

- Rust：94 tests passed；`cargo fmt --all -- --check`、`cargo test`、`cargo check` 通過。
- Node：13 tests passed；`public/app.js` 與 `public/i18n.js` 通過 `node --check`。
- M365 fail-fast：`--session`、`--model`、`--reasoning`、`--image`、`--file`、`--image-output` 均以 exit code 1 結束，且未啟動 9223 listener。
- M365 login：已登入 profile 執行 `ask-bridge --provider m365 login` 可自動偵測成功。
- M365 短查詢：同一 profile 連續 10 次，10/10 取得非空回覆與 Thread Link。
- M365 長查詢：25 項串流回覆完整擷取，最終文字長度 1501 字元。
- M365 Markdown：標題、清單、粗體、inline code、Rust code fence 與一般連結均由 Copy 路徑保留。
- DOM fallback：移除 Copy 按鈕後，仍可保留標題、清單、Rust fenced code block、程式碼、一般連結與 citation URL；code editor 行號未混入輸出。
- `--output`：檔案內容非空、與終端主要回覆一致，且不包含 Thread Link。
- `--new`：M365 page 數量增加一，既有 page ID 全部保留。
- timeout：`--timeout 1` 的長回覆以 exit code 1 結束，不輸出空成功或 Thread Link。
- 維護命令：M365 `open`、`get <url>` 成功；Windows `close` 增加 identity re-check 後的強制 fallback，實測 query 後可關閉 9223 listener。
- 未完成：zh-TW、macOS、第二帳號／租戶、MFA／Conditional Access、服務忙碌／內容政策，以及 ChatGPT／Gemini／Claude 完整真機矩陣。

## 10. Phase 2：穩定化、錯誤處理與發布面

### 10.1 語系與租戶差異

- [x] 在 en-US UI 完成登入、短答、長答、Markdown 與 `--new`。
- [ ] 在 zh-TW UI 完成相同流程。
- [ ] 比較兩個帳號或租戶的 selector 差異。
- [x] 只為實際觀察到的差異增加 fallback，不加入猜測性 selector。
- [ ] selector 失效時回報具體階段，不影響其他 provider。

### 10.2 跨平台

- [x] Windows：首次 Entra 登入。
- [ ] Windows：MFA。
- [x] Windows：已登入 headless query。
- [x] Windows：clipboard Copy 與 DOM fallback。
- [ ] Windows：登入逾時、服務錯誤、zh-TW 與 en-US。
- [ ] macOS：首次 Entra 登入。
- [ ] macOS：已登入 headless query。
- [ ] macOS：clipboard Copy 與 DOM fallback。
- [ ] macOS：`--new`、短答、長答與 Markdown。
- [ ] macOS：MFA、登入逾時與服務錯誤至少擇一驗證。
- [x] Linux／WSL 不列入第一版承諾；文件保留現有 Chrome 支援並註明企業登入政策需另行驗證。

### 10.3 錯誤與安全語意

- [x] LoggedOut：提示執行 M365 login。
- [x] Unknown：停止送出並提示以 headful／verbose 重試，不將 Unknown 當 LoggedIn。
- [ ] token 過期或 MFA 重新驗證：要求重新 headful login。
- [ ] Conditional Access／DLP：呈現實際政策阻擋摘要，不嘗試繞過。
- [ ] composer not found：指出可能為 UI rollout／selector 失效。
- [x] send button disabled timeout：指出 prompt 未送出。
- [x] no assistant message found：回傳失敗。
- [x] response timeout：回傳非成功狀態，不將空輸出視為回答。
- [ ] Copilot service busy／內容政策拒絕：擷取可見錯誤並分類。
- [x] verbose log 不輸出 access token、cookie、完整 HTML、登入表單、帳號、prompt 或回覆內容。
- [x] 使用者企業資料只依其既有 Microsoft 365 權限處理，不增加自動持久化。

### 10.4 文件與 metadata

必要檔案：

- [x] `README.md`
  - 新增 M365 experimental 支援、登入、config、query、`--new`、限制與安全提醒。
- [x] `README.en.md`
  - 與繁中 README 保持功能與限制一致。
- [x] `docs/quick-start.md`
  - 加入 M365 login/config/query 指令，並修正平台描述與支援 provider 列表。
- [x] `CHANGELOG.md`
  - 記錄新 provider、experimental 狀態、第一版能力與已知限制。
- [x] `package.json`
  - description 加入 Microsoft 365 Copilot。
  - keywords 加入 `microsoft-365`、`m365`、`copilot` 等可搜尋字詞。
- [x] `PRODUCT.md`
  - 更新 provider 支援與產品定位，不把企業資料能力描述成額外授權。

依正式支援面決定：

- [x] `scripts/ask.sh`
  - 確認 legacy wrapper 是否有 provider allowlist 或 help 文案；需要時更新。
  - 已確認此 wrapper 仍是只支援 ChatGPT／Gemini hash URL 的舊版瀏覽器開啟器，連 Claude 都不在其正式支援面，因此本次不加入無法驗證的 M365 auto-submit。
- [x] `public/index.html`、`public/app.js`
  - 官方網站顯示 M365 experimental 支援、範例與限制。
- [x] `skills/ask-bridge/SKILL.md`
  - 加入 M365 選擇、登入、適用任務與第一版未支援功能。
- [x] `src/model-selection.cjs` 與 `tests/model-selection.test.cjs`
  - 第一版不修改；只有 Phase 3 開放 M365 模式選擇時才變更。
- [x] `mcp_servers.json`
  - 維持不變，M365 繼續使用既有 Chrome DevTools MCP server。

### 10.5 文件一致性檢查

- [x] 文件命令可由實際 CLI parser 接受。
- [x] provider aliases、顯示名稱與 config 序列化規則一致。
- [x] 第一版所有未支援參數在 README、quick start 與 Skill 中明確標示。
- [x] Thread Link 不被描述為已支援 session resume。
- [x] 文件提醒企業資料仍受組織政策與 Microsoft 365 權限控制。
- [x] 不宣稱 Microsoft Graph API、work grounding 或 add-on 能力屬於第一版。

## 11. 詳細測試矩陣

### 11.1 Rust 單元測試

- [x] CLI 解析 `--provider m365`。
- [x] config aliases 解析並序列化為 `m365`。
- [x] provider precedence 不變。
- [x] `Provider::from_url` 只接受允許的 M365 host 與 HTTPS。
- [x] Entra、Office、相似 host 與自訂登入網域不被視為 M365 provider URL。
- [x] 第一版 M365 不接受 conversation URL 或 raw session ID。
- [x] M365 未支援參數均明確拒絕。
- [x] capability matrix 保留既有 provider 行為。
- [x] selector baseline 非空。
- [x] login signal script 包含必要訊號與穩定化。
- [x] composer-only 不足以確認 M365 登入。
- [x] URL 推論與 provider 衝突檢查不影響既有 provider。
- [x] 完成判定不接受空 response。
- [x] 回覆擷取不接受空白、工具列或重複來源內容。

### 11.2 JavaScript tests

第一版不修改 model selection helper。只有 Phase 3 實作 M365 picker 時才新增：

- [ ] M365 picker 偵測。
- [ ] 主標籤比對。
- [ ] selected state 驗證。
- [ ] 找不到選項時列出可用選項。

### 11.3 手動端到端矩陣

| 案例 | Windows | macOS |
|---|---:|---:|
| 首次 Entra 登入 | 必測 | 必測 |
| MFA 登入 | 必測 | 擇一 |
| 已登入 headless query | 必測 | 必測 |
| `--new` 新對話 | 必測 | 必測 |
| 短回覆 | 必測 | 必測 |
| 長串流回覆 | 必測 | 必測 |
| 標題、清單、code block、連結與 citation | 必測 | 必測 |
| Clipboard Copy | 必測 | 必測 |
| DOM scraper fallback | 必測 | 必測 |
| 登入逾時／token 過期 | 必測 | 擇一 |
| Conditional Access／MFA 重新驗證 | 擇一 | 擇一 |
| 服務忙碌／錯誤訊息 | 擇一 | 擇一 |
| zh-TW UI | 必測 | 擇一 |
| en-US UI | 必測 | 擇一 |

## 12. 建議實作切片與提交順序

下列功能切片已合併於 `ba32e96` 單一提交；勾選代表對應產出已完成，不代表實際採用逐筆提交順序。

1. [x] `test(m365): 記錄 DOM 探勘基線與驗收證據`
   - 完成 Phase 0，不提交敏感頁面內容。
2. [x] `refactor(provider): 建立集中式能力矩陣`
   - 只重構既有 provider，先以測試證明行為不變。
3. [x] `feat(m365): 新增 provider 與 CLI 契約`
   - variant、aliases、display、home URL、fail-fast。
4. [x] `feat(m365): 實作 URL 與登入狀態`
   - ownership、ready check、login signals、Entra redirect。
5. [x] `feat(m365): 實作文字提交與回覆等待`
   - selectors、submit、completion polling、空 response 保護。
6. [x] `feat(m365): 實作 Copy 與 DOM 回覆擷取`
   - Markdown、citation、fallback、非空成功條件。
7. [ ] `test(m365): 補齊單元與跨 provider 回歸`
   - Rust tests、條件式 Node tests、smoke test 紀錄。
8. [x] `docs(m365): 發布 experimental provider 文件`
   - README、quick start、CHANGELOG、PRODUCT、package、網站、Skill。

## 13. 風險、緩解與回滾

| 風險 | 等級 | 緩解 | 回滾／停用方式 |
|---|---:|---|---|
| M365 DOM 頻繁變更 | 高 | 語意 selector、多 fallback、smoke test、experimental 標記 | fail-closed 停用 M365，不影響其他 provider |
| Entra MFA／Conditional Access 阻擋 headless | 高 | login 強制 headful、重新驗證提示 | 要求人工重新登入，不做繞過 |
| 租戶／語系／rollout 差異 | 高 | 兩帳號、雙語系驗證 | 限縮支援聲明或延後發布 |
| 空 response container 先建立 | 中 | stop button + 非空文字穩定檢查 | timeout／擷取失敗，不回傳成功 |
| Copy／clipboard 不可靠 | 中 | DOM Markdown scraper fallback | 回傳可讀純文字；仍為空則失敗 |
| conversation URL 不穩定 | 中 | 第一版關閉 session | 只輸出目前 Thread Link，不承諾 resume |
| 企業資料誤寫入 log | 中 | 禁止 payload／HTML／token log | 移除敏感診斷，保留階段與錯誤分類 |
| 新 variant 破壞 exhaustive match | 中 | capability matrix、完整 Rust tests | revert M365 commits 或 feature disable |

## 14. Rollout 與監控

- [ ] 在 feature branch 完成 Phase 0 與 Phase 1。
- [ ] 由至少兩個 Microsoft 365 帳號執行 smoke test。
- [x] 文件、CLI help 與 changelog 均標示 experimental。
- [ ] 發布前確認既有 provider 全部通過回歸。
- [ ] 發布後監控下列錯誤分類：
  - `composer not found`
  - `send button disabled timeout`
  - `no assistant message found`
  - `response timeout`
  - `login state unknown`
  - `copy button not found`
  - `empty response`
- [x] selector 失效時快速讓 M365 fail-closed；不得以擴大通用 selector 的方式影響其他 provider。

## 15. Phase 3：選配功能逐項解鎖

每項功能各自建立規格、測試、文件與發布切片，不綁成一次性「完整支援」。

### 15.1 Session resume

- [x] 取得真實 conversation URL 格式。
- [x] 驗證重新開啟後仍為同一對話。
- [ ] 驗證 URL 是否含租戶或一次性狀態。
- [ ] 只有 ID 可穩定轉成 URL 時才開放 raw `--session-id`。
- [ ] 若只能可靠使用完整 URL，只開放 `--session-url`。
- [ ] 更新 capability、URL ownership、CLI 錯誤、文件與測試。

### 15.2 File attachment

- [ ] 優先使用可存取 upload button 與 MCP `upload_file`。
- [ ] 驗證檔名 chip、上傳完成、失敗與 DLP 錯誤。
- [ ] 第一批只評估 PDF、DOCX、TXT。
- [ ] 各格式均有獨立手動測試後才更新 capability。

### 15.3 Image attachment

- [ ] 驗證 hidden file input、DataTransfer 或 file chooser。
- [ ] 驗證預覽、上傳完成與移除。
- [ ] DLP／租戶政策拒絕時回報實際錯誤。
- [ ] 不與 file attachment 綁定發布。

### 15.4 Model／mode selection

- [ ] 只有 UI 明確允許切換且能驗證 selected state 時才實作。
- [x] 不將 Microsoft 自動選模推論為可用的 `--model`。
- [ ] 修改 `src/model-selection.cjs`。
- [ ] 更新 `tests/model-selection.test.cjs`。
- [ ] 找不到選項時列出實際可用選項。
- [ ] 更新 capability、README、quick start 與 Skill。

## 16. Microsoft Graph API 後續評估

此項不阻塞 UI provider，也不與 Phase 1–3 混合實作。

- [ ] 建立 `Provider -> BrowserProvider transport -> GraphCopilot transport` 重構提案。
- [ ] 評估 Entra app registration 與 delegated permission。
- [ ] 評估 device code／localhost redirect OAuth。
- [ ] 設計安全 token cache。
- [ ] 設計 tenant、consent、throttling 與 `retry-after` 錯誤處理。
- [ ] 實作 SSE parser POC。
- [ ] 評估 conversation ID 保存與恢復。
- [ ] 評估 citation、Adaptive Card 與 Markdown 轉換。
- [ ] 明確標示 `/beta` API、add-on 授權與 production SLA 限制。

## 17. 最終發布檢查

- [x] Phase 0 探勘證據與退出條件完成。
- [x] 第一版功能矩陣逐項符合實際行為。
- [x] 未支援功能全部 fail-fast。
- [x] 短答成功率達 9/10 以上。
- [x] 長串流與 Markdown 回覆完整且非空。
- [x] LoggedOut／Unknown 均不會送出 prompt。
- [ ] Windows 與 macOS 必測矩陣完成。
- [ ] ChatGPT、Gemini、Claude 回歸完成。
- [x] `cargo fmt --all -- --check`
- [x] `cargo test`
- [x] `cargo check`
- [x] `npm test`
- [x] README、quick start、CHANGELOG、PRODUCT、package metadata、網站與 Skill 一致。
- [x] 安全與隱私要求完成審查。
- [ ] Rollout、監控與 selector 失效停用策略已準備。
