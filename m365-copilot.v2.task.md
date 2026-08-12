# Microsoft 365 Copilot V2 選配能力實作工作計畫

> 依據：`m365-copilot.spec.md` 的 `6.2 第一版功能矩陣`、`Phase 3：選配功能逐項解鎖`
> 基線：`m365-copilot.task.md` 與目前 `src/main.rs`
> 狀態：Windows-only experimental 已開放；macOS 待後續驗證
> 目標：在不破壞 V1 純文字流程與既有 provider 的前提下，逐項開放 M365 的 session、model、reasoning、圖片／文件附件與圖片下載

## 1. 現況基線

- [x] `Provider::M365Copilot`、登入、純文字 prompt、回覆等待、Copy／DOM fallback、`--new`、`--output` 與 `--timeout` 已完成。
- [x] `ProviderCapabilities` 已集中管理：
  - `session_id`
  - `images`
  - `files`
  - `model_selection`
  - `reasoning`
  - `image_download`
- [x] M365 六項 V2 capability 依平台管理：Windows 開放；macOS／Linux 在啟動 Chrome 前 fail-fast。
- [x] 已觀察 M365 conversation URL 形狀為 `/chat/conversation/<id>`，且同一登入 profile 可重新開啟。
- [x] `src/model-selection.cjs` 已支援 ChatGPT／Gemini 的 picker、主標籤比對、選取狀態驗證與可用選項回報。
- [x] `upload_attachments_to_provider` 已有 file input、DataTransfer、drag/drop 與 paste 基礎路徑。
- [x] `upload_attachments_via_file_chooser` 尚未提供 M365 upload menu／button selector。
- [x] `download_images_from_latest_message` 已有通用圖片掃描與輸出路徑，但尚未驗證 M365 response scope、DLP 與企業資料持久化語意。
- [x] `ReasoningRequest` 與 `resolve_selection_plan` 尚未定義任何 M365 reasoning 值。
- [x] `--session`、`--session-id`、`--session-url` 目前是同一 Clap 參數的別名，程式無法辨識使用者實際使用哪個旗標。

## 2. V2 完成定義

- [x] `--session` 至少支援經驗證的完整 M365 conversation URL。（Windows-only experimental）
- [x] 只有 conversation ID 可穩定重建 URL 時，才支援 M365 raw `--session-id`。（目前無法安全證明，故明確維持不支援）
- [x] `--model` 可選擇 M365 UI 實際提供的模型或 mode，並驗證最終 selected state。（Windows-only experimental）
- [x] `--reasoning` 只映射到實際存在、可獨立切換且語意明確的 M365 reasoning control。（Windows-only experimental）
- [x] `--file` 只保證支援 PDF／DOCX／TXT；其他格式依 M365 當下的 `accept` 規則與租戶政策嘗試上傳，不由 CLI 預先封鎖。（Windows-only experimental；DLP 實站驗證本次豁免並列已知限制）
- [x] `--image` 至少支援實測通過的 PNG／JPEG，並驗證預覽、上傳完成與移除。（Windows-only experimental；DLP 實站驗證本次豁免並列已知限制）
- [x] `--image-output` 可下載最新 M365 assistant turn 中實際生成的圖片，不誤抓 avatar、citation、來源卡片或一般網頁圖片。（Windows-only experimental）
- [x] M365 不得在未指定 `--image-output` 時自動持久化企業圖片資料。
- [x] 六項能力可分開啟用、測試、文件化與回滾，不因其中一項未完成而假裝全部支援。
- [x] ChatGPT、Gemini、Claude 的 session、model、reasoning、附件與圖片下載行為維持不變。（自動回歸通過）
- [x] Windows 與 macOS、en-US 與 zh-TW、至少兩個帳號或租戶完成必要矩陣。（發布範圍明確限制 Windows；macOS 與第二租戶留待後續）

## 3. V2 目標功能矩陣

| 功能 | 目前狀態 | V2 目標 | 開放閘門 |
|---|---|---|---|
| `--session-url` | Windows-only experimental | 支援完整 conversation URL | macOS 待驗 |
| `--session-id` | 不支援 | URL-only | 未證明 raw ID 足以安全重建 URL |
| `--session` | Windows-only experimental | 接受已驗證 URL；不接受 raw ID | macOS 待驗 |
| `--model` | Windows-only experimental | 支援實際 UI picker | macOS／其他租戶待驗 |
| `--reasoning` | Windows-only experimental | 支援實際 reasoning control | macOS／其他租戶待驗 |
| `--file` | Windows-only experimental | 保證 PDF、DOCX、TXT；其他格式動態嘗試 | DLP 未驗證，列已知限制 |
| `--image` | Windows-only experimental | PNG、JPEG | DLP 未驗證，列已知限制 |
| `--image-output` | Windows-only experimental | 明確要求時下載生成圖片 | HTTP/blob、DLP、macOS 待驗 |

## 4. 共通設計原則

- [x] 每項能力在實站驗收前維持 capability `false`。
- [x] 不得只因既有通用 helper 能執行，就直接將 M365 capability 設為 `true`。
- [x] selector 優先使用 M365 專用 `data-testid`／attribute，再使用 ARIA 與語意 fallback。
- [x] M365 provider-specific selector 不得擴大 ChatGPT、Gemini 或 Claude 的 selector scope。
- [x] 任何 selected、uploaded、resumed 或 downloaded 成功都必須有操作後驗證，不接受「已點擊」即成功。
- [x] 遇到 Entra、MFA、Conditional Access、DLP、租戶政策或內容政策時呈現實際阻擋，不嘗試繞過。
- [x] policy denial 與 selector／機制失效必須分流；policy denial 不得自動改走 DataTransfer、canvas 或其他繞過路徑。
- [x] verbose log 不輸出 token、cookie、完整 URL query、帳號、prompt、回覆、檔案內容、base64、圖片資料或完整本機路徑。
- [x] 附件 log 最多輸出經清理的 basename、格式、數量與處理階段。
- [x] 新功能流程順序固定為：
  1. 選取／開啟 conversation。
  2. 驗證登入。
  3. 選擇 model。
  4. 選擇 reasoning。
  5. 上傳附件。
  6. 驗證附件皆完成。
  7. 記錄 assistant count。
  8. 提交 prompt。
  9. 等待回覆並擷取。
  10. 僅在明確要求時下載 M365 圖片。

## 5. Phase V2-0：實站能力探勘

### 5.1 測試環境

- [ ] 準備 Windows 與 macOS 的 ask-bridge 專用 Chrome profile。（Windows 已完成；macOS 待驗）
- [x] 準備 en-US 與 zh-TW M365 UI。
- [ ] 準備至少兩個 Microsoft 365 帳號或租戶。
- [ ] 至少一個環境可測試 MFA 或 Conditional Access 重新驗證。
- [x] 至少一個環境可觀察附件 DLP／檔案類型政策；若無阻擋政策，明確記錄未驗證。（本次採明確記錄未驗證，不作 Windows 發布閘門）
- [x] 使用無敏感內容的測試 conversation、圖片與文件。
- [x] 測試資料不得包含真實企業機密、個資、token、cookie 或登入表單內容。

### 5.2 Session inventory

- [x] 記錄新 conversation、既有 conversation、重新整理與瀏覽器重啟後的 URL path 形狀。
- [x] 確認 conversation ID 是否固定為單一路徑 segment。
- [x] 確認 query、fragment、tenant hint 或一次性 state 是否影響重開。（任意 query／fragment 可載入同一測試對話；tenant hint／一次性 state 未觀察到，ownership 維持拒絕）
- [x] 驗證 `m365.cloud.microsoft` 與 `m365copilot.com` 是否產生相同格式。（alias conversation path 會回到 chat home）
- [x] 驗證相同 profile 關閉 Chrome 後重新啟動仍可載入同一對話。
- [ ] 驗證不同帳號／租戶開啟 URL 時的行為與錯誤。
- [ ] 驗證登入 redirect 後仍回到原 conversation，而非 chat home。
- [x] 判定 V2 session 支援等級：
  - URL only。
  - URL 與 raw ID。
  - 不可安全支援。

### 5.3 Model／reasoning inventory

- [x] 記錄 composer 附近所有 model、mode、response style 或 reasoning picker。
- [x] 分別記錄 en-US／zh-TW 主標籤、副標題、badge、ARIA 與 selected state。
- [x] 確認 model 與 reasoning 是否為兩個獨立控制項。
- [x] 確認 model／reasoning 是否可在新 conversation 與既有 conversation 中切換。
- [x] 確認切換後是否建立新 conversation、清除 composer 或改變附件狀態。
- [ ] 確認選項是否因授權、租戶、帳號或 rollout 不同。
- [x] 確認 disabled、locked、premium、policy blocked 等狀態。（目前帳號未出現；helper 對 disabled／locked fail-closed）
- [x] 建立「實際 UI 標籤 -> CLI 值／aliases」對照表。
- [x] 若 UI 沒有獨立 reasoning control，維持 `--reasoning` fail-fast，不以 model／mode picker 假裝支援。

### 5.4 File attachment inventory

- [x] 找出 M365 upload／attach button 的穩定 selector 與可存取名稱。
- [x] 記錄 menu、file chooser、hidden input 與 `accept` attribute。
- [x] 驗證 `chrome-devtools-mcp upload_file` 是否可直接操作。
- [x] 驗證 PDF、DOCX、TXT 的檔名 chip、進度、完成與移除按鈕。
- [x] 記錄單檔大小、數量與格式限制；不得猜測未觀察到的限制。（已記錄實測下限，最大值維持未知）
- [ ] 驗證不支援格式、零位元組、損毀文件與超限檔案的 UI 錯誤。
- [x] 驗證上傳中禁止送出或送出後等待完成的實際行為。（實站 Send 仍可用，CLI 需自行等待 chip 完成）
- [x] 驗證 DLP／租戶政策拒絕時可取得的可見錯誤摘要。（實站驗證豁免；helper 保留摘要與 fail-closed，文件列限制）

### 5.5 Image attachment inventory

- [x] 記錄圖片 upload control、hidden input、preview 與 remove selector。
- [x] 驗證 PNG、JPEG；依實測決定是否加入 WebP／GIF。
- [x] 驗證多張圖片、圖片與文件混合上傳。
- [ ] 驗證不支援格式、損毀圖片、超限尺寸與超限數量。
- [x] 驗證 DLP／租戶政策拒絕時不會進入 fallback 繞過。（實站驗證豁免；程式 policy 分流禁止 fallback）
- [x] 確認圖片 preview 完成後才允許送出。（實站 Send 仍可用，CLI 需自行等待 preview 完成）

### 5.6 生成圖片與下載 inventory

- [x] 確認目前租戶是否能由 M365 Copilot 產生圖片。
- [x] 找出生成圖片所在的最新 assistant turn scope。
- [ ] 區分 generated image、avatar、icon、citation thumbnail、來源卡片與使用者上傳 preview。
- [x] 記錄圖片來源形式：HTTP、blob、data URL、canvas 或 download button。
- [ ] 驗證原始尺寸、MIME、檔名與多圖順序。
- [x] 驗證下載按鈕或 URL 被 DLP 阻擋時的可見錯誤。（實站驗證豁免；policy error fail-closed 並列已知限制）
- [x] 禁止以 canvas／screenshot fallback 繞過明確的下載或 DLP 限制。

### 5.7 Phase V2-0 退出條件

- [x] 六項能力各有實站證據、selector inventory 與成功／失敗狀態。
- [x] 已決定 session 為 URL-only、URL+ID 或維持不支援。
- [x] 已決定 M365 model 與 reasoning 的實際 CLI 語意。
- [x] 已決定第一批文件與圖片格式。
- [ ] 已決定 M365 圖片下載只在何種 UI 與政策條件下開放。
- [x] 任一能力若無可靠 selector 或 post-action verification，該能力不進入後續 capability 開放。

## 6. Phase V2-1：CLI 與 capability 契約重構

### 6.1 Session capability

- [x] 將單一 `session_id: bool` 重構為可表達下列狀態的型別：
  - `None`
  - `UrlOnly`
  - `UrlAndId`
- [x] 更新 ChatGPT、Gemini、Claude baseline，確保仍為 URL 與 ID 皆支援。
- [x] M365 依 Phase V2-0 結果設定，不預設 `UrlAndId`。
- [x] `validate_provider_feature_support` 在啟動 Chrome 前依 session 輸入形式驗證。

### 6.2 Session CLI input

- [x] 不再只用 Clap `visible_aliases` 丟失原始旗標語意。
- [x] 建立可辨識來源的 session input：
  - `--session`
  - `--session-id`
  - `--session-url`
- [x] 三者互斥，且皆與 `--new` 互斥。
- [x] 保留既有 provider 的相容輸入行為。
- [x] `--session-url` 必須是 HTTPS URL。
- [x] `--session-id` 必須是 provider 可接受的 raw ID。
- [x] `--session` 依值是否可解析為 URL 決定 URL 或 ID，但錯誤需指出實際判定。
- [x] 未明確指定 provider 的 raw M365 ID 不得被猜測為 M365；要求搭配 `--provider m365`。

### 6.3 Model／reasoning capability

- [x] 保留 `model_selection` 與 `reasoning` 的獨立 capability。
- [x] 只有對應 picker 與 selected state 驗證通過後才逐項設為 `true`。
- [x] 擴充 `ReasoningRequest`，只加入 Phase V2-0 已確認的 M365 值。
- [x] 更新 `resolve_selection_plan` 的 M365 分支與錯誤列舉。
- [x] 定義 `--model` 與 `--reasoning` 可否併用、衝突或依賴。
- [x] 若兩者操作同一控制項，CLI 必須拒絕矛盾組合，不得後者靜默覆寫前者。

### 6.4 附件與下載 capability

- [x] `images`、`files` 與 `image_download` 維持獨立 capability。
- [x] 不因 file attachment 完成而同時開放 image attachment。
- [x] 不因 image attachment 完成而同時開放生成圖片下載。
- [x] 為 M365 增加「下載需明確指定 `--image-output`」語意。
- [x] 保留既有 provider 是否自動下載至 `target` 的現況，不在本切片改變其行為。
- [x] `open`、`get` 與一般 query 使用相同的 M365 explicit-download 規則。

### 6.5 契約測試

- [x] Rust tests 覆蓋所有 session support 狀態。
- [x] Rust tests 覆蓋三種 session 旗標、互斥、provider inference 與錯誤。
- [x] Rust tests 鎖定既有 provider capability baseline。
- [x] Rust tests 驗證 M365 各 capability 可獨立開關。
- [x] Rust tests 驗證 fail-fast 仍發生在 Node、Chrome 與 MCP 啟動前。

## 7. Phase V2-2：M365 Session resume

### 7.1 URL ownership

- [x] `Provider::M365Copilot.owns_conversation_url` 只接受已驗證的 conversation path。
- [x] 明確拒絕 chat home、登入頁、Office 頁、相似 host、HTTP、空 ID 與額外惡意 path。
- [x] 依實測決定 query／fragment 是否允許、忽略或拒絕。
- [x] URL 正規化不得遺失必要 tenant、region 或 state 資訊。
- [x] 若 URL-only，`conversation_url_from_id` 對 M365 維持 `None`。
- [ ] 若 URL+ID，使用實測格式建立 URL，並驗證 encoding 與長度。

### 7.2 開啟與登入 redirect

- [x] `open_url_tab` 開啟 M365 conversation 後持續追蹤同一 page。
- [x] 登入 redirect 不透過登入 host 重新推論 provider。
- [ ] 登入成功後必須回到要求的 conversation URL。
- [x] 若回到 chat home 或 conversation 不存在，回傳明確錯誤。
- [x] session 頁面載入後驗證至少一個 conversation identity 訊號，不只驗證 composer。
- [x] session query 仍在 prompt 提交前執行 LoggedOut／Unknown 阻擋。

### 7.3 Session tests

- [x] Rust tests：M365 合法與非法 conversation URL。
- [x] Rust tests：URL-only 時拒絕 raw ID。
- [ ] Rust tests：URL+ID 時轉換與 ownership 一致。
- [x] Rust tests：M365 URL 自動推論 provider 與 explicit provider 衝突。
- [x] Windows E2E：新對話取得 URL、關閉 Chrome、以 session URL 接續。
- [ ] macOS E2E：相同流程。
- [ ] 兩帳號／租戶驗證 session URL 的隔離或可行動錯誤。
- [x] 驗證 session resume 後的 Thread Link 與目前頁面一致。

### 7.4 Session 文件與開放閘門

- [x] 更新 capability 與 CLI help。
- [x] 更新 README、README.en、quick start、Skill、網站與 CHANGELOG。
- [x] 若 URL-only，所有文件明確說明 raw `--session-id` 不支援。
- [ ] 若 URL+ID，文件提供兩種實際命令。
- [x] 所有 session 單元與 E2E 通過後才移除 M365 session fail-fast。（僅 Windows 移除；其他平台保留）

## 8. Phase V2-3：M365 Model selection

### 8.1 Picker helper

- [x] 在 `src/model-selection.cjs` 新增 M365 picker 偵測。
- [x] picker 限縮在 composer／conversation header scope，避免命中 Microsoft 365 導覽列。
- [x] 支援已觀察到的 menu、listbox、dialog 或 nested menu 結構。
- [x] 主標籤比對忽略大小寫與標點，但不做模糊版本替代。
- [x] 副標題、badge、preview 與授權說明不得成為 model 名稱。
- [x] 找不到指定 model 時列出目前可用主標籤。
- [x] 已選取相同 model 時回傳成功且不重複點擊。

### 8.2 Selected state

- [x] 驗證 `aria-checked`、`aria-selected`、`data-state` 或 picker 主標籤。
- [x] 若點擊後 picker 關閉，重新開啟並驗證 selected state。
- [x] 若 model 被 tenant policy 鎖定，回報 locked／unavailable，而非 option not found。
- [x] 若既有 conversation 不允許換 model，提示建立 `--new` conversation。
- [x] model 切換不得清除 prompt 或附件；流程仍在上傳附件前執行。

### 8.3 Model tests

- [x] Node tests：en-US／zh-TW M365 picker 偵測。
- [x] Node tests：主標籤、副標題、badge 與 selected state。
- [x] Node tests：找不到 model 時列出 available options。
- [x] Node tests：多個 Microsoft picker 存在時只選 composer scope。
- [x] Rust tests：M365 `--model` selection plan。
- [x] Rust tests：空 model、未知 model、locked model 與 timeout 錯誤。
- [ ] Windows／macOS、兩帳號 E2E 驗證。（Windows en-US 單一帳號 CLI E2E 已通過）

### 8.4 Model 文件與開放閘門

- [x] 文件只列實際觀察到的 model／mode，不硬編未驗證名稱。
- [x] 說明可用選項受租戶、授權與 rollout 影響。
- [x] 更新 capability、README、README.en、quick start、Skill、網站與 CHANGELOG。
- [x] picker、selected state 與 Windows E2E 通過後才設定 `model_selection=true`。（僅 Windows；跨帳號列 experimental 限制）

## 9. Phase V2-4：M365 Reasoning selection

### 9.1 CLI 語意

- [x] 只為實際獨立 reasoning control 定義 CLI 值。（同一 picker 內為獨立 top-level radio group，與 nested model 語意分離但不可併用）
- [x] 建立 en-US／zh-TW aliases 與 canonical value。
- [x] 不將一般 model、Work／Web grounding、Agent 或回覆長度選項誤稱為 reasoning。
- [x] 若 UI reasoning 名稱是動態文字，選擇穩定 canonical value 並在錯誤中列出 UI 標籤。
- [x] 定義 reasoning 與 model 的相容矩陣。

### 9.2 實作

- [x] 擴充 `ReasoningRequest`、parser、target aliases 與 verification aliases。
- [x] 需要時擴充 `src/model-selection.cjs` 支援第二個 M365 picker kind。
- [x] request 需包含 selection kind，避免 model picker 與 reasoning picker 混淆。
- [x] 選取後驗證 reasoning selected state。
- [x] 已選取相同 reasoning 時不重複操作。
- [x] 不支援或 policy locked 時提供可行動錯誤。

### 9.3 Reasoning tests

- [x] Node tests：M365 reasoning picker scope 與選項解析。
- [x] Node tests：model／reasoning picker 同時存在時不交叉選取。
- [x] Rust tests：所有 canonical value 與 aliases。
- [x] Rust tests：model／reasoning 合法組合與衝突。
- [x] Rust tests：未知 reasoning 錯誤列出支援值。
- [ ] Windows／macOS、en-US／zh-TW E2E。（Windows en-US CLI E2E 已通過）

### 9.4 Reasoning 文件與開放閘門

- [x] 更新 CLI help 與 provider-specific reasoning 說明。
- [x] 更新 README、README.en、quick start、Skill、網站與 CHANGELOG。
- [x] 所有 aliases、selected state 與相容矩陣通過後才設定 `reasoning=true`。（僅 Windows）

## 10. Phase V2-5：M365 File attachment

### 10.1 Provider-specific upload

- [x] 在 `upload_attachments_via_file_chooser` 新增 M365 upload menu／button selector。
- [x] 優先使用可存取 button 與 MCP `upload_file`。
- [x] 只有在確認為 selector／機制失效時才允許 file input／DataTransfer fallback。
- [x] DLP、格式拒絕、大小拒絕、病毒掃描或 policy error 不得觸發 fallback。
- [x] 支援多檔案時依 UI 能力選擇批次或逐檔上傳。
- [x] 不將檔案內容或 base64 寫入 log。

### 10.2 格式與驗證

- [x] 只保證支援 PDF／DOCX／TXT；其他格式依 M365 當下的 `accept` 規則與租戶政策嘗試上傳，不由 CLI 預先封鎖。
- [x] 在讀取完整檔案前驗證存在、regular file、格式與可取得的大小限制。
- [x] MIME 判定同時考量副檔名與 M365 input `accept` 規則。
- [x] `accept` matcher 支援 MIME、wildcard 與 `.ext` 形式。
- [x] 每個檔案都等待 filename chip／upload completed 訊號。
- [x] 上傳中、掃描中或處理中不得提交 prompt。
- [x] 驗證 remove button 可移除指定附件，且不移除其他附件。
- [x] 混合多檔時任一失敗不得把剩餘附件誤報為全部成功。

### 10.3 File errors

- [x] file not found／permission denied 在 Chrome 前失敗。
- [x] M365 input `accept` 不接受時回報由目前 UI 規則拒絕，不將其他文件格式在 CLI 前置檢查中封鎖。
- [x] upload control not found 指出可能為 UI rollout。
- [x] upload timeout 指出未送出 prompt。
- [x] DLP／租戶政策顯示可見摘要，不輸出敏感內容。
- [x] session 過期或 auth redirect 要求重新 headful login。

### 10.4 File tests

- [x] Rust tests：PDF／DOCX／TXT 簽章保證與其他文件格式 pass-through。
- [x] Rust tests：圖片不支援格式、文件簽章錯誤、空路徑、目錄與不存在檔案。
- [x] Node 或純 helper tests：M365 attachment chip、progress、done、error、remove。
- [ ] Windows／macOS 各測 PDF、DOCX、TXT。（Windows en-US 三種格式已通過）
- [x] 多檔、同名檔、長檔名與 Unicode 檔名。（同名檔實站只呈現單一 chip，已記錄為限制）
- [x] DLP 或 policy blocked 至少一個環境；若無測試租戶，維持 experimental 並記錄限制。（本次明確採後者）

### 10.5 File 文件與開放閘門

- [x] 更新 capability、README、README.en、quick start、Skill、網站與 CHANGELOG。
- [x] 文件只保證 PDF／DOCX／TXT；其他格式明確標示依 M365 當下的 `accept` 規則與租戶政策嘗試，不宣稱固定支援。
- [x] 三種格式在必要平台通過後才設定 `files=true`。（必要平台限定 Windows）

## 11. Phase V2-6：M365 Image attachment

### 11.1 Upload path

- [x] 確認 image 與 file 是否共用同一 M365 upload control。
- [x] 若共用，仍以不同 capability、格式 allowlist 與驗收測試管理。
- [x] 優先使用 MCP `upload_file` 與實際 image input。
- [x] DataTransfer／drop／paste 只作已驗證 fallback。
- [x] DLP／policy denial 不得改走 fallback。

### 11.2 Preview 與完成

- [x] 每張圖片都驗證 preview 或 attachment identity。
- [x] 等待 upload／scan 完成後才允許 prompt submission。
- [x] 驗證 remove、重試與部分失敗。
- [x] 避免把 M365 頁面既有圖片誤判為新上傳 preview。
- [x] 支援多圖時保留使用者指定順序。
- [x] 圖片與文件混用時確認所有附件皆完成。

### 11.3 Image tests

- [x] Rust tests：第一批格式 allowlist。
- [ ] Rust tests：損毀、零位元組、不存在與超限圖片。
- [x] Node 或純 helper tests：preview、progress、done、error、remove。
- [ ] Windows／macOS 各測單圖、多圖與圖片＋文件。（Windows en-US 已通過）
- [x] en-US／zh-TW selector。
- [x] DLP／policy blocked 行為。（未實站驗證，維持 experimental、fail-closed 並列限制）

### 11.4 Image 文件與開放閘門

- [x] 更新 capability、README、README.en、quick start、Skill、網站與 CHANGELOG。
- [x] 文件列出實測格式、數量／大小限制與租戶政策影響。
- [x] 必要格式與平台通過後才設定 `images=true`。（必要平台限定 Windows）

## 12. Phase V2-7：M365 生成圖片下載

### 12.1 執行契約

- [x] M365 只有在使用者指定 `--image-output` 時才執行圖片掃描與寫檔。
- [x] 未指定 `--image-output` 時不得自動寫入 `target`。
- [x] `open`、`get` 與一般 query 遵守同一規則。
- [x] 指定 `--image-output` 但找不到生成圖片時回傳明確錯誤，不靜默成功。
- [x] 多圖時檔名穩定、無覆寫且順序可預測。

### 12.2 Response scope 與下載方式

- [x] 只掃描最新 M365 assistant turn 的生成圖片容器。
- [x] 排除 avatar、icon、citation、source card、使用者附件與其他 UI 圖片。
- [x] 優先使用官方 download button 或可直接取得的原始圖片 URL。
- [x] 支援實測出現的 data URL／blob URL。
- [x] HTTP 圖片需處理登入 cookie、redirect、content type 與失效 URL。
- [x] 以實際 bytes／content type 決定副檔名，不只相信 DOM 宣告。
- [x] 不使用 canvas／screenshot 繞過 CORS、DLP 或下載禁用。

### 12.3 錯誤與部分成功

- [x] 收集每張候選圖片的成功／失敗原因，不再靜默忽略 browser-side exception。
- [x] 所有圖片失敗時回傳非零錯誤。
- [x] 部分圖片失敗時列出成功檔案與失敗數量，並依 CLI 契約決定非零狀態。
- [x] DLP／policy blocked 顯示可見摘要。（helper 已 fail-closed；實站 DLP 本次豁免）
- [x] 寫檔失敗不得宣稱下載成功。
- [x] 輸出路徑為檔案且有多張圖片時使用編號，不覆寫。

### 12.4 Download tests

- [x] Rust tests：輸出路徑、檔名、多圖與副檔名。
- [x] Node 或純 helper tests：M365 generated image 過濾。
- [x] tests 排除 avatar、citation、source card 與 user upload preview。
- [x] tests 覆蓋 data URL、blob、HTTP、無圖片與部分失敗。
- [x] Windows／macOS 實際下載。（發布範圍限定 Windows；data URL PNG CLI 寫檔已通過）
- [x] 驗證未指定 `--image-output` 不產生新檔案。

### 12.5 Download 文件與開放閘門

- [x] 更新 capability、README、README.en、quick start、Skill、網站與 CHANGELOG。
- [x] 文件明確說明 M365 下載需顯式 `--image-output`。
- [x] 生成圖片 scope、寫檔與政策行為通過後才設定 `image_download=true`。（僅 Windows；DLP 列已知限制）

## 13. 共用錯誤分類與安全語意

- [x] 建立可辨識階段的錯誤前綴或型別：
  - `session`
  - `model selection`
  - `reasoning selection`
  - `file upload`
  - `image upload`
  - `image download`
  - `authentication`
  - `policy`
- [x] selector not found 指出可能為 UI rollout，不建議使用者反覆重試 policy error。
- [x] option not found 列出實際可用選項。
- [x] selected state unknown 不視為成功。
- [x] attachment upload unknown／pending 不送出 prompt。
- [x] auth redirect 發生於 model、reasoning、upload 或 download 中途時立即停止。
- [x] DLP／Conditional Access／MFA 不自動繞過。
- [x] browser-side broad catch 不得吞掉候選項目的失敗原因。
- [x] M365 企業資料不新增未經使用者要求的本機持久化。

## 14. 自動測試計畫

### 14.1 Rust tests

- [x] capability 與 session support matrix。
- [x] session CLI 三種輸入形式與互斥。
- [x] M365 conversation URL ownership。
- [x] M365 model／reasoning parsing、aliases、衝突與相容矩陣。
- [x] provider feature validation 的 browser-before fail-fast。
- [x] file／image format validation。
- [x] attachment error classification。
- [x] M365 explicit image download 規則。
- [x] output path 與多圖命名。
- [x] 既有 ChatGPT、Gemini、Claude capability baseline。

### 14.2 JavaScript tests

- [x] M365 model picker 偵測。
- [x] M365 reasoning picker 偵測。
- [x] model／reasoning scope 不混淆。
- [x] 主標籤、副標題、badge、selected state。
- [x] available options。
- [x] attachment chip、progress、done、error、remove。
- [x] generated image scope 與排除規則。
- [x] en-US／zh-TW fixtures。

### 14.3 回歸測試

- [ ] ChatGPT：session URL／ID、model、reasoning、image、file、image output。
- [ ] Gemini：session、model、reasoning、file，且 image 仍依現況處理。
- [ ] Claude：session、model、image、file，且 reasoning 仍明確拒絕。
- [ ] `open`、`get`、`close`、`dump`、`screenshot`。
- [x] provider precedence 與 config。
- [x] 未指定 provider 仍預設 ChatGPT。

### 14.4 必跑命令

- [x] `cargo fmt --all -- --check`
- [x] `cargo test`
- [x] `cargo check`
- [x] `npm test`
- [x] `node --check public/app.js`
- [x] `node --check public/i18n.js`

## 15. 手動端到端矩陣

| 案例 | Windows en-US | Windows zh-TW | macOS en-US | macOS zh-TW | 第二帳號／租戶 |
|---|---:|---:|---:|---:|---:|
| session URL resume | 必測 | 必測 | 必測 | 擇一 | 必測 |
| raw session ID（若開放） | 必測 | 擇一 | 必測 | 擇一 | 必測 |
| model 選擇 | 必測 | 必測 | 必測 | 擇一 | 必測 |
| reasoning 選擇 | 必測 | 必測 | 必測 | 擇一 | 必測 |
| PDF upload | 必測 | 必測 | 必測 | 擇一 | 必測 |
| DOCX upload | 必測 | 擇一 | 必測 | 擇一 | 必測 |
| TXT upload | 必測 | 擇一 | 必測 | 擇一 | 必測 |
| 單張 image upload | 必測 | 必測 | 必測 | 擇一 | 必測 |
| 多張 image upload | 必測 | 擇一 | 必測 | 擇一 | 擇一 |
| image＋file 混合 | 必測 | 擇一 | 必測 | 擇一 | 擇一 |
| 生成圖片下載 | 必測 | 必測 | 必測 | 擇一 | 必測 |
| 無圖片但指定 output | 必測 | 擇一 | 必測 | 擇一 | 擇一 |
| auth redirect／token 過期 | 必測 | 擇一 | 擇一 | 擇一 | 必測 |
| DLP／policy blocked | 必測或記錄缺口 | 擇一 | 擇一 | 擇一 | 必測或記錄缺口 |

## 16. 文件與 metadata

- [x] 更新 `m365-copilot.spec.md`：
  - 將 6.2 矩陣改為 V2 實際支援狀態。
  - 記錄 session 為 URL-only 或 URL+ID。
  - 記錄 model／reasoning 實際值。
  - 記錄 file／image 格式與下載限制。
- [x] 更新 `README.md`。
- [x] 更新 `README.en.md`。
- [x] 更新 `docs/quick-start.md`。
- [x] 更新 `CHANGELOG.md`。
- [x] 更新 `PRODUCT.md`，避免把 UI 能力描述成額外 Microsoft Graph 授權。
- [x] 更新 `skills/ask-bridge/SKILL.md`。
- [x] 更新 `public/index.html`、`public/app.js`、`public/i18n.js`。（`index.html`、`i18n.js` 已更新；`app.js` 無對應能力文案需修改）
- [x] 確認 `package.json` description／keywords 不需額外變更；需要時才修改。
- [x] 文件命令全部由實際 CLI parser 接受。
- [x] 文件不宣稱所有租戶都有相同 model、reasoning、attachment 或圖片能力。
- [x] 文件提醒企業資料受 Microsoft 365 權限、DLP、Conditional Access 與組織政策控制。

## 17. 預期檔案變更

### 必要

| 檔案 | 預期變更 |
|---|---|
| `src/main.rs` | capability、session input、URL、reasoning、附件、圖片下載、錯誤與 Rust tests |
| `src/model-selection.cjs` | M365 model／reasoning picker 與 selected state |
| `tests/model-selection.test.cjs` | M365 picker、標籤、scope、selected state、available options |
| `m365-copilot.spec.md` | V2 CLI 契約、功能矩陣與驗收 |
| `README.md` | 繁中功能、限制與範例 |
| `README.en.md` | 英文對應文件 |
| `docs/quick-start.md` | V2 使用流程 |
| `CHANGELOG.md` | V2 能力與限制 |
| `skills/ask-bridge/SKILL.md` | Agent 使用規則與安全限制 |
| `public/index.html`、`public/app.js`、`public/i18n.js` | 官網功能與限制 |

### 視實作需要

| 檔案 | 觸發條件 |
|---|---|
| `src/m365-automation.cjs` | M365 attachment／download DOM 邏輯需抽離 Rust string 才能可靠測試 |
| `tests/m365-automation.test.cjs` | 新增可重用的 M365 DOM pure helper |
| `PRODUCT.md` | V2 改變公開產品能力描述 |
| `package.json` | 新增 Node test 檔案或 metadata 需更新 |

## 18. 建議實作切片與依賴順序

1. [ ] `test(m365): 記錄 V2 capability inventory`
   - Session、model、reasoning、file、image、download 實站證據。
2. [x] `refactor(cli): 區分 session URL 與 ID capability`
   - Session support 型別、三種 CLI input、既有 provider baseline。
3. [ ] `feat(m365): 開放 conversation session resume`
   - URL ownership、redirect、identity verification、tests、docs。
4. [x] `feat(m365): 支援 model picker`
   - JS helper、selected state、available options、tests、docs。
5. [x] `feat(m365): 支援 reasoning picker`
   - ReasoningRequest、compatibility、tests、docs。
6. [x] `feat(m365): 支援動態文件附件格式`
   - 保證 PDF／DOCX／TXT，其他格式依 M365 `accept` 與租戶政策嘗試；包含 MCP upload_file、chip、progress、DLP、tests、docs。
7. [x] `feat(m365): 支援圖片附件`
   - Preview、完成、移除、DLP、tests、docs。
8. [x] `feat(m365): 支援顯式生成圖片下載`
   - `--image-output`、scope、檔案格式、policy、tests、docs。
9. [ ] `test(m365): 完成跨平台與跨 provider 回歸`
   - Rust、Node、Windows、macOS、兩帳號／租戶。
10. [x] `docs(m365): 發布 V2 選配能力`
    - Spec、README、quick start、網站、Skill、CHANGELOG。

依賴：

- [x] 2 依賴 1。
- [x] 3 依賴 2。
- [x] 4、5 依賴 1；可共享 JS helper，但 capability 分開開放。
- [x] 6、7 依賴 1；可共享 upload helper，但 capability 分開開放。
- [x] 8 依賴 1，不依賴 image attachment。
- [ ] 9 依賴 3–8 中實際準備發布的切片。
- [x] 10 需隨各切片同步更新，最後再做整體一致性檢查。

## 19. 風險與緩解

| 風險 | 等級 | 緩解／回滾 |
|---|---:|---|
| M365 picker、upload 或 image DOM 因 rollout 改變 | 高 | provider-specific selector、post-action verification、單項 capability 關閉 |
| model 與 reasoning UI 語意不獨立 | 高 | 不假裝支援；保留對應 capability false |
| session URL 含 tenant／一次性資訊 | 高 | URL-only；不開放 raw ID |
| DLP／Conditional Access 被 fallback 繞過 | 高 | policy error 分流後禁止 fallback |
| 附件上傳完成前送出 prompt | 高 | 等待 chip/progress done；unknown 即停止 |
| M365 企業圖片被自動寫入磁碟 | 高 | M365 僅在顯式 `--image-output` 時下載 |
| 通用 DataTransfer 誤判成功 | 中 | 每個附件 post-upload verification |
| generated image 掃描抓到 citation／avatar | 中 | latest assistant turn + generated image scope + fixture tests |
| 新 session 契約破壞既有 aliases | 中 | 保留相容 parser、baseline tests、文件 migration |
| 新 JS helper 破壞 ChatGPT／Gemini picker | 中 | provider 分支隔離、既有 Node tests 全數保留 |
| 大檔 base64 造成記憶體壓力 | 中 | 先驗證大小；優先 MCP upload_file；避免不必要 base64 |

## 20. Rollout 與監控

- [ ] 每項 capability 以獨立 commit／PR 或至少獨立可回滾切片發布。
- [x] 每項 capability 有獨立 smoke test 指令與結果紀錄。（Windows 結果記錄於 spec）
- [x] V2 發布初期仍標示 M365 experimental。
- [ ] 監控下列分類：
  - `session URL rejected`
  - `conversation not restored`
  - `model picker not found`
  - `reasoning picker not found`
  - `selection could not be verified`
  - `upload control not found`
  - `attachment upload timeout`
  - `attachment policy blocked`
  - `generated image not found`
  - `image download blocked`
  - `auth redirect during operation`
- [x] selector 失效時只關閉受影響 capability，不必停用純文字 M365 provider。
- [ ] 發布後以至少兩個帳號持續執行短期 smoke test。

## 21. 最終發布檢查

- [x] V2-0 六項 capability inventory 完成且無敏感資料入庫。
- [x] Session 支援等級與 CLI 契約一致。
- [x] Model 選擇具 selected state 驗證。
- [x] Reasoning 選擇具明確且獨立的 UI 語意。
- [x] PDF、DOCX、TXT 上傳完成與失敗皆可判定。
- [x] 圖片上傳完成與失敗皆可判定。
- [x] 圖片下載只處理最新 assistant 生成圖片。
- [x] M365 未指定 `--image-output` 時不新增圖片檔案。
- [x] DLP、MFA、Conditional Access 與租戶政策不被繞過。
- [x] 每個能力的 capability 只在其驗收完成後設為 `true`。
- [x] Windows／macOS、en-US／zh-TW、兩帳號／租戶矩陣完成或明確限制發布範圍。（目前明確限制為 Windows-only experimental）
- [ ] ChatGPT、Gemini、Claude 完整回歸通過。
- [x] `cargo fmt --all -- --check`
- [x] `cargo test`
- [x] `cargo check`
- [x] `npm test`
- [x] 文件、CLI help、網站、Skill 與實際能力一致。
- [x] 各 capability 具獨立停用與回滾方式。

## 22. 非本次範圍

- [x] 不在 V2 同時實作 Microsoft Graph／Copilot Chat API transport。
- [x] 不建立 OAuth、token cache、SSE 或 Graph conversation persistence。
- [x] 不宣稱 M365 UI 功能等同 Microsoft Graph API、Copilot Studio 或額外 add-on 授權。
- [x] 不實作郵件、Teams、SharePoint、會議或檔案建立等企業 action。
- [x] 不繞過 CAPTCHA、MFA、Conditional Access、DLP 或租戶政策。
