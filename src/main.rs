use base64::{Engine as _, engine::general_purpose};
use clap::{ArgAction, CommandFactory, Parser, Subcommand, ValueEnum};
use mcp_cli::{McpClient, McpConnection, ServerConfig, StdioClient};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::io::{self, IsTerminal, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const ASK_BRIDGE_CHROME_MARKER: &str = "--ask-bridge-instance";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoginState {
    LoggedIn,
    LoggedOut,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct LoginSignals {
    account: bool,
    auth_control: bool,
    auth_path: bool,
    composer: bool,
    stable: bool,
}

impl LoginSignals {
    fn state(self, provider: Provider) -> LoginState {
        if self.auth_path {
            LoginState::LoggedOut
        } else if self.account {
            LoginState::LoggedIn
        } else if !self.stable {
            LoginState::Unknown
        } else if self.auth_control {
            LoginState::LoggedOut
        } else if self.composer && provider == Provider::ChatGpt {
            LoginState::LoggedIn
        } else {
            LoginState::Unknown
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Provider {
    #[value(name = "chatgpt")]
    ChatGpt,
    #[value(name = "gemini")]
    Gemini,
    #[value(name = "claude")]
    Claude,
    #[value(name = "m365")]
    M365Copilot,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionSupport {
    None,
    UrlOnly,
    UrlAndId,
}

impl SessionSupport {
    fn supports_url(self) -> bool {
        matches!(self, SessionSupport::UrlOnly | SessionSupport::UrlAndId)
    }

    fn supports_id(self) -> bool {
        self == SessionSupport::UrlAndId
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProviderCapabilities {
    session: SessionSupport,
    images: bool,
    files: bool,
    model_selection: bool,
    reasoning: bool,
    image_download: bool,
}

impl Provider {
    fn from_config_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "chatgpt" | "chat-gpt" | "chat_gpt" => Some(Provider::ChatGpt),
            "gemini" => Some(Provider::Gemini),
            "claude" | "claude-ai" | "claude_ai" | "claudeai" => Some(Provider::Claude),
            "m365" | "m365-copilot" | "m365_copilot" | "microsoft365" | "microsoft-365-copilot" => {
                Some(Provider::M365Copilot)
            }
            _ => None,
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Provider::ChatGpt => "ChatGPT",
            Provider::Gemini => "Gemini",
            Provider::Claude => "Claude",
            Provider::M365Copilot => "Microsoft 365 Copilot",
        }
    }

    fn home_url(self) -> &'static str {
        match self {
            Provider::ChatGpt => "https://chatgpt.com/",
            Provider::Gemini => "https://gemini.google.com/app",
            Provider::Claude => "https://claude.ai/new",
            Provider::M365Copilot => "https://m365.cloud.microsoft/chat",
        }
    }

    fn capabilities(self) -> ProviderCapabilities {
        self.capabilities_for_platform(cfg!(target_os = "windows"))
    }

    fn capabilities_for_platform(self, is_windows: bool) -> ProviderCapabilities {
        match self {
            Provider::ChatGpt => ProviderCapabilities {
                session: SessionSupport::UrlAndId,
                images: true,
                files: true,
                model_selection: true,
                reasoning: true,
                image_download: true,
            },
            Provider::Gemini => ProviderCapabilities {
                session: SessionSupport::UrlAndId,
                images: false,
                files: true,
                model_selection: true,
                reasoning: true,
                image_download: true,
            },
            Provider::Claude => ProviderCapabilities {
                session: SessionSupport::UrlAndId,
                images: true,
                files: true,
                model_selection: true,
                reasoning: false,
                image_download: true,
            },
            Provider::M365Copilot => ProviderCapabilities {
                session: if is_windows {
                    SessionSupport::UrlOnly
                } else {
                    SessionSupport::None
                },
                images: is_windows,
                files: is_windows,
                model_selection: is_windows,
                reasoning: is_windows,
                image_download: is_windows,
            },
        }
    }

    fn owns_url(self, url: &str) -> bool {
        Self::from_url(url) == Some(self)
    }

    fn from_url(url: &str) -> Option<Self> {
        let parsed = Url::parse(url).ok()?;
        if parsed.scheme() != "https" {
            return None;
        }

        match parsed.host_str()?.to_ascii_lowercase().as_str() {
            "chatgpt.com" | "www.chatgpt.com" => Some(Provider::ChatGpt),
            "gemini.google.com" => Some(Provider::Gemini),
            "claude.ai" | "www.claude.ai" => Some(Provider::Claude),
            "m365.cloud.microsoft"
            | "www.m365.cloud.microsoft"
            | "m365copilot.com"
            | "www.m365copilot.com" => Some(Provider::M365Copilot),
            _ => None,
        }
    }

    fn conversation_url_from_id(self, session_id: &str) -> Option<String> {
        match self {
            Provider::ChatGpt => Some(format!("https://chatgpt.com/c/{session_id}")),
            Provider::Gemini => Some(format!("https://gemini.google.com/app/{session_id}")),
            Provider::Claude => Some(format!("https://claude.ai/chat/{session_id}")),
            Provider::M365Copilot => None,
        }
    }

    fn owns_conversation_url(self, url: &Url) -> bool {
        if Self::from_url(url.as_str()) != Some(self) {
            return false;
        }

        let path_segments: Vec<&str> = url
            .path_segments()
            .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
            .unwrap_or_default();
        match self {
            Provider::ChatGpt => path_segments
                .windows(2)
                .any(|segments| segments[0] == "c" && !segments[1].is_empty()),
            Provider::Gemini => path_segments
                .windows(2)
                .any(|segments| segments[0] == "app" && !segments[1].is_empty()),
            Provider::Claude => path_segments
                .windows(2)
                .any(|segments| segments[0] == "chat" && !segments[1].is_empty()),
            Provider::M365Copilot => {
                url.username().is_empty()
                    && url.password().is_none()
                    && url.port().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none()
                    && matches!(
                        url.host_str().map(str::to_ascii_lowercase).as_deref(),
                        Some("m365.cloud.microsoft" | "www.m365.cloud.microsoft")
                    )
                    && path_segments.len() == 3
                    && path_segments[0] == "chat"
                    && path_segments[1] == "conversation"
                    && valid_session_id(path_segments[2])
            }
        }
    }

    fn ready_check_js(self) -> &'static str {
        match self {
            Provider::ChatGpt => r#"() => document.getElementById('prompt-textarea') !== null"#,
            Provider::Gemini => {
                r#"() => {
                    return document.querySelector('div[role="textbox"][aria-label*="Gemini"]') !== null ||
                           document.querySelector('rich-textarea [contenteditable="true"]') !== null ||
                           document.querySelector('.ql-editor[contenteditable="true"]') !== null ||
                           document.querySelector('a[href*="accounts.google.com"]') !== null ||
                           /Sign in|登入/.test(document.body.innerText || '');
                }"#
            }
            Provider::Claude => {
                r#"() => {
                    return document.querySelector('div[contenteditable="true"][data-testid="chat-input"]') !== null ||
                           document.querySelector('div[contenteditable="true"].ProseMirror') !== null ||
                           document.querySelector('[data-testid="login-with-google"]') !== null ||
                           window.location.pathname.startsWith('/login') ||
                           /Sign in|登入/.test(document.body.innerText || '');
                }"#
            }
            Provider::M365Copilot => {
                r#"() => {
                    const host = window.location.hostname.toLowerCase();
                    return host === 'login.microsoftonline.com' ||
                           host === 'login.live.com' ||
                           document.querySelector('#m365-chat-editor-target-element') !== null ||
                           document.querySelector('#user-account-avatar') !== null ||
                           document.querySelector('input[name="loginfmt"]') !== null ||
                           document.querySelector('#idSIButton9') !== null;
                }"#
            }
        }
    }

    fn login_signals_js(self) -> &'static str {
        match self {
            Provider::ChatGpt => {
                r#"async () => {
                    const isVisible = (el) => {
                        if (!el) return false;
                        const style = window.getComputedStyle(el);
                        const rect = el.getBoundingClientRect();
                        return style.display !== 'none' &&
                            style.visibility !== 'hidden' &&
                            style.opacity !== '0' &&
                            rect.width > 0 &&
                            rect.height > 0;
                    };

                    const textFor = (el) => [
                        el.getAttribute('aria-label'),
                        el.getAttribute('title'),
                        el.textContent
                    ].filter(Boolean).join(' ').trim();

                    const readSignals = () => {
                        const visibleAuthButton = Array.from(document.querySelectorAll('a, button'))
                            .some((el) => {
                                if (!isVisible(el)) return false;
                                const text = textFor(el);
                                return /^(log in|login|sign in|sign up|登入|登錄|登录|註冊|注册)$/i.test(text);
                            });

                        const composer = document.querySelector('#prompt-textarea') ||
                            document.querySelector('[data-testid="composer-text-input"]') ||
                            document.querySelector('textarea[placeholder*="Message"]') ||
                            document.querySelector('textarea[placeholder*="訊息"]') ||
                            document.querySelector('[contenteditable="true"]');

                        const accountMenu = document.querySelector('[data-testid="profile-button"]') ||
                            document.querySelector('[data-testid="account-menu-button"]') ||
                            document.querySelector('[data-testid="user-menu-button"]') ||
                            document.querySelector('button[aria-label*="Profile"]') ||
                            document.querySelector('button[aria-label*="profile"]') ||
                            document.querySelector('button[aria-label*="Account"]') ||
                            document.querySelector('button[aria-label*="account"]') ||
                            document.querySelector('button[aria-label*="User"]') ||
                            document.querySelector('button[aria-label*="user"]') ||
                            document.querySelector('button[aria-label*="帳戶"]') ||
                            document.querySelector('button[aria-label*="使用者"]');

                        return {
                            account: isVisible(accountMenu),
                            auth_control: Boolean(visibleAuthButton),
                            auth_path: /\/(auth|login|signup)(\/|$)/i.test(window.location.pathname),
                            composer: isVisible(composer)
                        };
                    };

                    let signals = readSignals();
                    let signature = JSON.stringify(signals);
                    const startedAt = Date.now();
                    let stableSince = startedAt;
                    let stable = false;
                    const earliestDecision = startedAt + 2000;
                    const deadline = Date.now() + 5000;
                    while (!signals.account && !signals.auth_path && Date.now() < deadline) {
                        await new Promise((resolve) => setTimeout(resolve, 250));
                        const nextSignals = readSignals();
                        const nextSignature = JSON.stringify(nextSignals);
                        if (nextSignature !== signature) {
                            signature = nextSignature;
                            stableSince = Date.now();
                        }
                        signals = nextSignals;
                        if (Date.now() >= earliestDecision && Date.now() - stableSince >= 750) {
                            stable = true;
                            break;
                        }
                    }

                    return { ...signals, stable };
                }"#
            }
            Provider::Gemini => {
                r#"() => {
                    const isVisible = (el) => {
                        if (!el) return false;
                        const style = window.getComputedStyle(el);
                        const rect = el.getBoundingClientRect();
                        return style.display !== 'none' &&
                            style.visibility !== 'hidden' &&
                            style.opacity !== '0' &&
                            rect.width > 0 &&
                            rect.height > 0;
                    };
                    const composer = document.querySelector('div[role="textbox"][aria-label*="Gemini"]') ||
                        document.querySelector('rich-textarea [contenteditable="true"]') ||
                        document.querySelector('.ql-editor[contenteditable="true"]');
                    const accountEl = document.querySelector('a[href*="accounts.google.com/SignOutOptions"]') ||
                        document.querySelector('a[aria-label*="Google 帳戶"]') ||
                        document.querySelector('a[aria-label*="Google Account"]');
                    const hasAccount = accountEl && (accountEl.href?.includes('SignOutOptions') || accountEl.closest('header') !== null);
                    const signIn = Array.from(document.querySelectorAll('a, button'))
                        .some((el) => isVisible(el) && /Sign in|登入/.test([
                                el.getAttribute('aria-label'),
                                el.textContent
                            ].filter(Boolean).join(' ')));
                    const authPath = /\/(auth|login|signin|signup)(\/|$)/i.test(window.location.pathname);
                    return {
                        account: Boolean(hasAccount),
                        auth_control: Boolean(signIn),
                        auth_path: authPath,
                        composer: Boolean(composer),
                        stable: true
                    };
                }"#
            }
            Provider::Claude => {
                r#"() => {
                    const isVisible = (el) => {
                        if (!el) return false;
                        const style = window.getComputedStyle(el);
                        const rect = el.getBoundingClientRect();
                        return style.display !== 'none' &&
                            style.visibility !== 'hidden' &&
                            style.opacity !== '0' &&
                            rect.width > 0 &&
                            rect.height > 0;
                    };
                    const composer = document.querySelector('div[contenteditable="true"][data-testid="chat-input"]') ||
                        document.querySelector('div[contenteditable="true"].ProseMirror');
                    const account = document.querySelector('[data-testid="user-menu-button"]') ||
                        document.querySelector('button[aria-label*="User menu"]') ||
                        document.querySelector('button[aria-label*="Account"]');
                    const signIn = document.querySelector('[data-testid="login-with-google"]') ||
                        Array.from(document.querySelectorAll('a, button'))
                            .find((el) => isVisible(el) && /^(log in|login|sign in|sign up|登入|註冊)$/i.test([
                                    el.getAttribute('aria-label'),
                                    el.textContent
                                ].filter(Boolean).join(' ').trim()));
                    const authPath = /^\/(login|signup|magic-link)(\/|$)/i.test(window.location.pathname);
                    return {
                        account: isVisible(account),
                        auth_control: Boolean(signIn),
                        auth_path: authPath,
                        composer: Boolean(composer),
                        stable: true
                    };
                }"#
            }
            Provider::M365Copilot => {
                r#"async () => {
                    const isVisible = (el) => {
                        if (!el) return false;
                        const style = window.getComputedStyle(el);
                        const rect = el.getBoundingClientRect();
                        return style.display !== 'none' &&
                            style.visibility !== 'hidden' &&
                            style.opacity !== '0' &&
                            rect.width > 0 &&
                            rect.height > 0;
                    };

                    const textFor = (el) => [
                        el.getAttribute('aria-label'),
                        el.getAttribute('title'),
                        el.value,
                        el.textContent
                    ].filter(Boolean).join(' ').trim();

                    const readSignals = () => {
                        const host = window.location.hostname.toLowerCase();
                        const authPath = host === 'login.microsoftonline.com' ||
                            host === 'login.live.com' ||
                            /\/(auth|login|signin|oauth2)(\/|$)/i.test(window.location.pathname);
                        const composer = document.querySelector('#m365-chat-editor-target-element') ||
                            document.querySelector('[role="textbox"][contenteditable="true"][aria-label*="Copilot"]');
                        const account = document.querySelector('#user-account-avatar');
                        const signIn = document.querySelector('input[name="loginfmt"]') ||
                            document.querySelector('#idSIButton9') ||
                            Array.from(document.querySelectorAll('a, button, input[type="submit"]'))
                                .find((el) => isVisible(el) &&
                                    /^(sign in|log in|登入|登錄|登录)$/i.test(textFor(el)));

                        return {
                            account: isVisible(account),
                            auth_control: isVisible(signIn),
                            auth_path: authPath,
                            composer: isVisible(composer)
                        };
                    };

                    let signals = readSignals();
                    let signature = JSON.stringify(signals);
                    const startedAt = Date.now();
                    let stableSince = startedAt;
                    let stable = false;
                    const earliestDecision = startedAt + 1000;
                    const deadline = startedAt + 5000;
                    while (!signals.account && !signals.auth_path && Date.now() < deadline) {
                        await new Promise((resolve) => setTimeout(resolve, 250));
                        const nextSignals = readSignals();
                        const nextSignature = JSON.stringify(nextSignals);
                        if (nextSignature !== signature) {
                            signature = nextSignature;
                            stableSince = Date.now();
                        }
                        signals = nextSignals;
                        if (Date.now() >= earliestDecision && Date.now() - stableSince >= 750) {
                            stable = true;
                            break;
                        }
                    }

                    return { ...signals, stable };
                }"#
            }
        }
    }

    fn assistant_selector(self) -> &'static str {
        match self {
            Provider::ChatGpt => "[data-message-author-role=\"assistant\"], .agent-turn",
            Provider::Gemini => "model-response",
            Provider::Claude => ".font-claude-response",
            Provider::M365Copilot => "[data-testid=\"copilot-message-div\"]",
        }
    }

    fn latest_response_selector(self) -> &'static str {
        match self {
            Provider::ChatGpt => {
                "[data-message-author-role=\"assistant\"], .agent-turn, model-response, .model-response, [data-test-id*=\"response\"], [data-testid*=\"response\"]"
            }
            Provider::Gemini => "model-response",
            Provider::Claude => ".font-claude-response",
            Provider::M365Copilot => "[data-testid=\"copilot-message-div\"]",
        }
    }

    fn response_content_selector(self) -> &'static str {
        match self {
            Provider::ChatGpt => "",
            Provider::Gemini => {
                "message-content, .markdown, structured-content-container.model-response-text"
            }
            Provider::Claude => ".standard-markdown, .font-claude-response-body",
            Provider::M365Copilot => {
                "[data-testid=\"markdown-reply\"], [data-testid=\"lastChatMessage\"]"
            }
        }
    }

    fn composer_selectors_json(self) -> &'static str {
        match self {
            Provider::ChatGpt => r##"["#prompt-textarea"]"##,
            Provider::Gemini => {
                r#"[
                    "div[role=\"textbox\"][aria-label*=\"Gemini\"]",
                    "rich-textarea [contenteditable=\"true\"]",
                    ".ql-editor[contenteditable=\"true\"]"
                ]"#
            }
            Provider::Claude => {
                r#"[
                    "div[contenteditable=\"true\"][data-testid=\"chat-input\"]",
                    "div[contenteditable=\"true\"].ProseMirror",
                    "div[aria-label*=\"Claude\"][contenteditable=\"true\"]"
                ]"#
            }
            Provider::M365Copilot => {
                r##"[
                    "#m365-chat-editor-target-element",
                    "[role=\"textbox\"][contenteditable=\"true\"][aria-label*=\"Copilot\"]"
                ]"##
            }
        }
    }

    fn send_button_selectors_json(self) -> &'static str {
        match self {
            Provider::ChatGpt => {
                r##"[
                    "[data-testid=\"send-button\"]",
                    "#composer-submit-button",
                    "button[aria-label*=\"Send\"]",
                    "button[aria-label*=\"傳送\"]",
                    "button[aria-label*=\"发送\"]"
                ]"##
            }
            Provider::Gemini => {
                r#"[
                    "button[aria-label=\"傳送訊息\"]",
                    "button[aria-label=\"Submit\"]",
                    "button[aria-label*=\"Send\"]",
                    "button[aria-label*=\"傳送\"]",
                    "button[aria-label*=\"提交\"]"
                ]"#
            }
            Provider::Claude => {
                r#"[
                    "button[aria-label=\"Send message\"]",
                    "button[aria-label*=\"Send\"]",
                    "button[aria-label*=\"傳送\"]"
                ]"#
            }
            Provider::M365Copilot => {
                r##"[
                    "#m365-chat-input-shared-container button[type=\"submit\"][aria-label=\"Send\"]",
                    "#m365-chat-input-shared-container button[type=\"submit\"][aria-label*=\"傳送\"]",
                    "#m365-chat-input-shared-container button[type=\"submit\"][aria-label*=\"送出\"]"
                ]"##
            }
        }
    }

    fn stop_button_selectors_json(self) -> &'static str {
        match self {
            Provider::ChatGpt => {
                r##"[
                    "[data-testid=\"stop-button\"]",
                    "#composer-stop-button",
                    "button[aria-label=\"Stop generating\"]"
                ]"##
            }
            Provider::Gemini => {
                r#"[
                    "button[aria-label=\"停止回覆\"]",
                    "button[aria-label*=\"Stop\"]",
                    "button[aria-label*=\"停止\"]"
                ]"#
            }
            Provider::Claude => {
                r#"[
                    "button[aria-label=\"Stop response\"]",
                    "button[aria-label*=\"Stop\"]",
                    "button[aria-label*=\"停止\"]"
                ]"#
            }
            Provider::M365Copilot => {
                r##"[
                    "#m365-chat-input-shared-container button[type=\"submit\"][aria-label=\"Stop generating\"]",
                    "#m365-chat-input-shared-container button[type=\"submit\"][aria-label*=\"Stop\"]",
                    "#m365-chat-input-shared-container button[type=\"submit\"][aria-label*=\"停止\"]",
                    "#m365-chat-input-shared-container button[type=\"submit\"][aria-label*=\"取消\"]"
                ]"##
            }
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Provider::ChatGpt => write!(f, "chatgpt"),
            Provider::Gemini => write!(f, "gemini"),
            Provider::Claude => write!(f, "claude"),
            Provider::M365Copilot => write!(f, "m365"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReasoningRequest {
    ChatGptAuto,
    ChatGptInstant,
    ChatGptMedium,
    ChatGptHigh,
    GeminiExtended,
    M365Auto,
    M365Quick,
    M365ThinkDeeper,
}

impl ReasoningRequest {
    fn target_aliases(self) -> &'static [&'static str] {
        match self {
            ReasoningRequest::ChatGptAuto => &["auto", "自動", "智慧"],
            ReasoningRequest::ChatGptInstant => &["instant", "即時"],
            ReasoningRequest::ChatGptMedium => &["medium", "中", "中等"],
            ReasoningRequest::ChatGptHigh => &["high", "高"],
            ReasoningRequest::GeminiExtended => &["extended thinking", "延伸思考"],
            ReasoningRequest::M365Auto => &["auto", "自動"],
            ReasoningRequest::M365Quick => &["quick response", "快速回應", "快速回应"],
            ReasoningRequest::M365ThinkDeeper => &["think deeper", "深度思考"],
        }
    }

    fn verification_aliases(self) -> &'static [&'static str] {
        match self {
            ReasoningRequest::GeminiExtended => {
                &["extended thinking", "延伸思考", "pro extended", "pro 延伸"]
            }
            _ => self.target_aliases(),
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SelectionPlan {
    model: Option<String>,
    reasoning: Option<ReasoningRequest>,
    used_legacy_model: bool,
}

fn normalize_option_label(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn parse_chatgpt_reasoning(value: &str) -> Option<ReasoningRequest> {
    match normalize_option_label(value).as_str() {
        "auto" | "自動" | "智慧" => Some(ReasoningRequest::ChatGptAuto),
        "instant" | "即時" => Some(ReasoningRequest::ChatGptInstant),
        "medium" | "中" | "中等" => Some(ReasoningRequest::ChatGptMedium),
        "high" | "高" => Some(ReasoningRequest::ChatGptHigh),
        _ => None,
    }
}

fn parse_gemini_reasoning(value: &str) -> Option<ReasoningRequest> {
    match normalize_option_label(value).as_str() {
        "extended" | "extendedthinking" | "延伸思考" | "proextended" | "pro延伸" => {
            Some(ReasoningRequest::GeminiExtended)
        }
        _ => None,
    }
}

fn parse_m365_reasoning(value: &str) -> Option<ReasoningRequest> {
    match normalize_option_label(value).as_str() {
        "auto" | "自動" => Some(ReasoningRequest::M365Auto),
        "quick" | "quickresponse" | "快速回應" | "快速回应" => {
            Some(ReasoningRequest::M365Quick)
        }
        "deep" | "deeper" | "thinkdeeper" | "深度思考" => {
            Some(ReasoningRequest::M365ThinkDeeper)
        }
        _ => None,
    }
}

fn canonical_m365_model(value: &str) -> Option<&'static str> {
    match normalize_option_label(value).as_str() {
        "gpt56" => Some("GPT 5.6"),
        "gpt55" => Some("GPT 5.5"),
        "sonnet" | "claudesonnet" => Some("Sonnet"),
        "opus" | "claudeopus" => Some("Opus"),
        _ => None,
    }
}

fn is_gemini_pro_model(model: &str) -> bool {
    let normalized = normalize_option_label(model);
    if normalized == "pro" {
        return true;
    }

    normalized.strip_suffix("pro").is_some_and(|version| {
        !version.is_empty() && version.chars().all(|character| character.is_ascii_digit())
    })
}

fn resolve_selection_plan(
    provider: Provider,
    model: Option<&str>,
    reasoning: Option<&str>,
) -> Result<SelectionPlan, String> {
    let raw_model = model.map(str::trim);
    if raw_model == Some("") {
        return Err("Empty model name".to_string());
    }
    let legacy_reasoning = match (provider, raw_model) {
        (Provider::ChatGpt, Some(value)) => parse_chatgpt_reasoning(value),
        (Provider::Gemini, Some(value)) => parse_gemini_reasoning(value),
        (Provider::M365Copilot, Some(value)) => parse_m365_reasoning(value),
        (Provider::Claude, _) | (_, None) => None,
    };
    let model = match (provider, raw_model) {
        (_, Some(value)) if legacy_reasoning.is_some() => Some(value.to_string()),
        (Provider::M365Copilot, Some(value)) => Some(
            canonical_m365_model(value)
                .ok_or_else(|| {
                    format!(
                        "Unsupported Microsoft 365 Copilot model '{value}'. Observed values: GPT 5.6, GPT 5.5, Sonnet, Opus"
                    )
                })?
                .to_string(),
        ),
        (_, value) => value.map(str::to_string),
    };

    let raw_reasoning = reasoning.map(str::trim);
    if raw_reasoning == Some("") {
        return Err("Empty reasoning value".to_string());
    }

    let explicit_reasoning = match (provider, raw_reasoning) {
        (_, None) => None,
        (Provider::ChatGpt, Some(value)) => Some(parse_chatgpt_reasoning(value).ok_or_else(|| {
            format!(
                "Unsupported ChatGPT reasoning '{value}'. Supported values: auto, instant, medium, high"
            )
        })?),
        (Provider::Gemini, Some(value)) => Some(parse_gemini_reasoning(value).ok_or_else(|| {
            format!("Unsupported Gemini reasoning '{value}'. Supported value: extended")
        })?),
        (Provider::Claude, Some(_)) => {
            return Err(
                "Claude does not support --reasoning; use --model for Sonnet, Opus, or Haiku"
                    .to_string(),
            );
        }
        (Provider::M365Copilot, Some(value)) => {
            Some(parse_m365_reasoning(value).ok_or_else(|| {
                format!(
                    "Unsupported Microsoft 365 Copilot reasoning '{value}'. Observed values: auto, quick, think-deeper"
                )
            })?)
        }
    };

    if explicit_reasoning.is_some() && legacy_reasoning.is_some() {
        return Err(
            "A reasoning-like --model value cannot be combined with --reasoning; move the reasoning value to --reasoning"
                .to_string(),
        );
    }

    let (model, reasoning, used_legacy_model) = if let Some(legacy) = legacy_reasoning {
        (None, Some(legacy), true)
    } else {
        (model, explicit_reasoning, false)
    };

    if provider == Provider::Gemini
        && reasoning == Some(ReasoningRequest::GeminiExtended)
        && model
            .as_deref()
            .is_some_and(|value| !is_gemini_pro_model(value))
    {
        return Err(
            "Gemini Extended Thinking is incompatible with non-Pro models; omit --model or select a Pro model"
                .to_string(),
        );
    }

    if provider == Provider::M365Copilot && model.is_some() && reasoning.is_some() {
        return Err(
            "Microsoft 365 Copilot model and reasoning options share one UI control and cannot be combined safely; choose either --model or --reasoning"
                .to_string(),
        );
    }

    Ok(SelectionPlan {
        model,
        reasoning,
        used_legacy_model,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct ChatGptAgentPrompt<'a> {
    agent_mention: &'a str,
    body: &'a str,
}

fn parse_chatgpt_agent_prompt(prompt: &str) -> Option<ChatGptAgentPrompt<'_>> {
    let rest = prompt.strip_prefix('@')?;
    let mut agent_chars = 0usize;

    for (idx, ch) in rest.char_indices() {
        if ch.is_whitespace() {
            if agent_chars == 0 || agent_chars > 10 {
                return None;
            }

            let body = rest[idx + ch.len_utf8()..].trim_start_matches(char::is_whitespace);
            if body.is_empty() {
                return None;
            }

            return Some(ChatGptAgentPrompt {
                agent_mention: &prompt[..idx + 1],
                body,
            });
        }

        agent_chars += 1;
        if agent_chars > 10 {
            return None;
        }
    }

    None
}

#[derive(Parser)]
#[command(name = "ask-bridge")]
#[command(version = "0.2.10")]
#[command(disable_version_flag = true)]
#[command(about = "AI browser CLI - Ask ChatGPT, Gemini, Claude or Microsoft 365 Copilot from your Terminal with your subscription", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// The prompt to send to the selected provider.
    /// If standard input is piped and this value is present, they are combined as:
    /// `prompt + "\\n\\n" + stdin`.
    prompt: Option<String>,

    /// AI provider to automate. Overrides ~/.config/ask-bridge/config.json.
    #[arg(long, short = 'p', value_enum, global = true)]
    provider: Option<Provider>,

    /// Run Chrome in headless mode. Defaults to true.
    #[arg(long, require_equals = true, num_args = 0..=1, default_value = "true", default_missing_value = "true")]
    headless: bool,

    /// Create a brand new provider session in a new tab while preserving existing tabs.
    #[arg(long, default_value_t = false)]
    new: bool,

    /// Resume an existing conversation by provider session ID or full conversation URL.
    /// M365 accepts full URLs only on Windows (experimental).
    #[arg(
        long = "session",
        value_name = "URL_OR_ID",
        conflicts_with_all = ["new", "session_id", "session_url"]
    )]
    session: Option<String>,

    /// Resume an existing conversation by raw provider session ID.
    /// M365 raw session IDs are unsupported.
    #[arg(
        long = "session-id",
        value_name = "ID",
        conflicts_with_all = ["new", "session", "session_url"]
    )]
    session_id: Option<String>,

    /// Resume an existing conversation by full HTTPS conversation URL.
    /// M365 support is Windows-only experimental.
    #[arg(
        long = "session-url",
        value_name = "URL",
        conflicts_with_all = ["new", "session", "session_id"]
    )]
    session_url: Option<String>,

    /// Print version information.
    #[arg(
        long = "version",
        short = 'v',
        short_alias = 'V',
        action = ArgAction::Version
    )]
    _version: (),

    /// Print verbose debugging status messages.
    #[arg(long, default_value_t = false)]
    verbose: bool,

    /// Write the final response in Markdown format to the specified file.
    #[arg(long, short, value_name = "FILE")]
    output: Option<String>,

    /// Write the downloaded images to the specified folder or file path.
    /// M365 support is Windows-only experimental and requires this explicit option.
    #[arg(long, short = 'i', value_name = "IMAGE_PATH")]
    image_output: Option<String>,

    /// Attach one or more local image files to the prompt (can be specified multiple times).
    /// M365 supports PNG/JPEG on Windows (experimental).
    #[arg(long = "image", value_name = "IMAGE_FILE", num_args = 1)]
    images: Vec<String>,

    /// Attach one or more local document files (PDF, Word, Excel, text, etc.) to the prompt
    /// (can be specified multiple times). M365 guarantees PDF/DOCX/TXT and dynamically tries
    /// other formats accepted by its UI on Windows (experimental).
    #[arg(long = "file", value_name = "FILE", num_args = 1)]
    files: Vec<String>,

    /// Maximum time in seconds to wait for the provider response.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..))]
    timeout: u64,

    /// Switch the provider model before sending the prompt.
    /// Match the primary menu label case- and punctuation-insensitively;
    /// subtitles and badges are ignored. M365 support is Windows-only experimental.
    #[arg(long = "model", value_name = "MODEL")]
    model: Option<String>,

    /// Select provider-specific reasoning separately from the model.
    /// ChatGPT: auto, instant, medium, high. Gemini: extended.
    /// M365 on Windows (experimental): auto, quick, think-deeper. Claude: unsupported.
    #[arg(long = "reasoning", value_name = "REASONING")]
    reasoning: Option<String>,
}

#[derive(Subcommand, Clone)]
enum Commands {
    /// Open Chrome browser, optionally navigate to a URL, and copy the latest response
    #[command(hide = true)]
    Open {
        /// Optional conversation URL to open before copying the latest response.
        url: Option<String>,
    },
    /// Retrieve the latest response from the selected provider (defaults to headless)
    #[command(hide = true)]
    Get {
        /// Optional conversation URL to fetch before copying the latest response.
        url: Option<String>,
        /// Print verbose debugging status messages.
        #[arg(long, default_value_t = false)]
        verbose: bool,
    },
    /// Open Chrome browser and wait for manual login
    Login,
    /// Close the managed Chrome browser instance
    Close,
    /// Set or show the global default provider used when --provider is not specified.
    Config,
    /// Reinstall ask-bridge using the recommended README installation command
    Update,
    /// Dump the current browser tab HTML for debugging
    #[command(hide = true)]
    Dump,
    /// Take a screenshot of the current browser tab for debugging
    #[command(hide = true)]
    Screenshot,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AppConfig {
    provider: Option<String>,
}

fn config_file_path() -> Result<PathBuf, String> {
    let mut config_path = home::home_dir().ok_or("Could not locate home directory")?;
    config_path.push(".config/ask-bridge/config.json");
    Ok(config_path)
}

fn parse_configured_provider(content: &str) -> Result<Option<Provider>, String> {
    let config: AppConfig =
        serde_json::from_str(content).map_err(|e| format!("Failed to parse config.json: {}", e))?;

    match config.provider {
        Some(provider) => Provider::from_config_value(&provider)
            .map(Some)
            .ok_or_else(|| format!("Invalid provider in config.json: {}", provider)),
        None => Ok(None),
    }
}

fn load_configured_provider() -> Result<Option<Provider>, String> {
    let config_path = config_file_path()?;
    if !config_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&config_path).map_err(|e| {
        format!(
            "Failed to read config file {}: {}",
            config_path.to_string_lossy(),
            e
        )
    })?;

    parse_configured_provider(&content).map_err(|e| {
        format!(
            "{}. Expected format: {{\"provider\":\"chatgpt\"}}, {{\"provider\":\"gemini\"}}, {{\"provider\":\"claude\"}}, or {{\"provider\":\"m365\"}}",
            e
        )
    })
}

fn effective_provider(
    cli_provider: Option<Provider>,
    configured_provider: Option<Provider>,
) -> Provider {
    cli_provider
        .or(configured_provider)
        .unwrap_or(Provider::ChatGpt)
}

fn resolve_provider_with<F>(
    cli_provider: Option<Provider>,
    load_provider: F,
) -> Result<Provider, String>
where
    F: FnOnce() -> Result<Option<Provider>, String>,
{
    if let Some(provider) = cli_provider {
        return Ok(provider);
    }

    Ok(effective_provider(None, load_provider()?))
}

fn resolve_provider(cli_provider: Option<Provider>) -> Result<Provider, String> {
    resolve_provider_with(cli_provider, load_configured_provider)
}

fn write_global_provider_config(provider: Provider) -> Result<(), String> {
    let config_path = config_file_path()?;
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create config directory {}: {}",
                parent.to_string_lossy(),
                e
            )
        })?;
    }

    let content =
        serde_json::to_string_pretty(&serde_json::json!({"provider": provider.to_string()}))
            .map_err(|e| format!("Failed to serialize provider config: {}", e))?;
    std::fs::write(&config_path, format!("{}\n", content)).map_err(|e| {
        format!(
            "Failed to write config file {}: {}",
            config_path.to_string_lossy(),
            e
        )
    })?;

    println!(
        "Set default provider to '{}' in {}",
        provider,
        config_path.to_string_lossy()
    );

    Ok(())
}

fn run_config_command(cli_provider: Option<Provider>) -> Result<(), String> {
    match cli_provider {
        Some(provider) => write_global_provider_config(provider),
        None => {
            let config_path = config_file_path()?;
            let configured_provider = load_configured_provider()?;
            match configured_provider {
                Some(provider) => {
                    println!("Current default provider: {}", provider);
                }
                None => {
                    println!("No default provider configured.");
                    println!("The effective provider is ChatGPT.");
                }
            }
            if config_path.exists() {
                println!("Config file: {}", config_path.to_string_lossy());
            } else {
                println!(
                    "Config file not created yet: {}",
                    config_path.to_string_lossy()
                );
            }
            println!(
                "Set default provider with: ask-bridge config --provider <chatgpt|gemini|claude|m365>"
            );
            println!("This is a one-time override example: ask-bridge --provider gemini <prompt>");
            Ok(())
        }
    }
}

fn run_update_command() -> Result<(), String> {
    println!("Running ask-bridge update via official installer...");
    println!("Progress: downloading installer and updating binary.");

    #[cfg(target_os = "windows")]
    let status = {
        let current_exe = std::env::current_exe()
            .map_err(|e| format!("Failed to locate current executable path: {}", e))?;
        let update_exe = current_exe
            .parent()
            .ok_or_else(|| "Failed to determine ask-bridge executable directory".to_string())?
            .join("ask-bridge-update.exe");

        if update_exe.exists() {
            let child = Command::new(update_exe)
                .arg(format!("--parent-pid={}", std::process::id()))
                .arg("--wait-seconds=30")
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|e| format!("Failed to launch ask-bridge-update.exe: {}", e))?;
            println!("Progress: updater started with PID {}.", child.id());
            println!("Progress: update command is running in background.");
            return Ok(());
        }

        println!("ask-bridge-update.exe not found. Falling back to inline installer.");
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "irm https://raw.githubusercontent.com/doggy8088/ask-bridge/main/install.ps1 | iex",
            ])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| format!("Failed to run Windows update command: {}", e))?
    };

    #[cfg(not(target_os = "windows"))]
    let status = Command::new("sh")
        .args([
            "-c",
            "curl -fsSL https://raw.githubusercontent.com/doggy8088/ask-bridge/main/install.sh | bash",
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("Failed to run macOS/Linux update command: {}", e))?;

    if status.success() {
        println!("Progress: update command completed.");
        Ok(())
    } else {
        Err(format!("Update command failed with exit status {}", status))
    }
}

struct Page {
    id: usize,
    url: String,
    selected: bool,
}

fn unique_new_page_id(before: &[Page], after: &[Page]) -> Result<usize, String> {
    let new_page_ids: Vec<usize> = after
        .iter()
        .filter(|candidate| !before.iter().any(|page| page.id == candidate.id))
        .map(|page| page.id)
        .collect();

    match new_page_ids.as_slice() {
        [page_id] => Ok(*page_id),
        [] => {
            Err("Could not identify the newly opened tab; existing tabs were preserved".to_string())
        }
        _ => Err(format!(
            "Could not uniquely identify the newly opened tab (new page IDs: {:?}); existing tabs were preserved",
            new_page_ids
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionInputKind {
    Auto,
    Id,
    Url,
}

impl SessionInputKind {
    fn flag_name(self) -> &'static str {
        match self {
            SessionInputKind::Auto => "--session",
            SessionInputKind::Id => "--session-id",
            SessionInputKind::Url => "--session-url",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SessionInput<'a> {
    kind: SessionInputKind,
    value: &'a str,
}

fn session_input(cli: &Cli) -> Option<SessionInput<'_>> {
    cli.session
        .as_deref()
        .map(|value| SessionInput {
            kind: SessionInputKind::Auto,
            value,
        })
        .or_else(|| {
            cli.session_id.as_deref().map(|value| SessionInput {
                kind: SessionInputKind::Id,
                value,
            })
        })
        .or_else(|| {
            cli.session_url.as_deref().map(|value| SessionInput {
                kind: SessionInputKind::Url,
                value,
            })
        })
}

fn valid_session_id(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 256
        && session
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn resolve_session_url_provider(
    selected_provider: Provider,
    provider_was_explicit: bool,
    url: &Url,
) -> Result<Provider, String> {
    let session_provider = Provider::from_url(url.as_str()).ok_or_else(|| {
        "Session URL must use HTTPS and belong to a supported provider host".to_string()
    })?;
    if !session_provider.owns_conversation_url(url) {
        return Err(format!(
            "session: URL is not a supported {} conversation URL",
            session_provider.display_name()
        ));
    }
    if provider_was_explicit && session_provider != selected_provider {
        return Err(format!(
            "Session URL belongs to {}, but --provider selected {}",
            session_provider.display_name(),
            selected_provider.display_name()
        ));
    }
    Ok(session_provider)
}

fn resolve_session_target(
    selected_provider: Provider,
    provider_was_explicit: bool,
    input: SessionInput<'_>,
) -> Result<(Provider, String), String> {
    let session = input.value.trim();
    if session.is_empty() {
        return Err("Session ID or URL cannot be empty".to_string());
    }

    let parsed_url = Url::parse(session).ok();
    let treat_as_url = match input.kind {
        SessionInputKind::Auto => parsed_url.is_some(),
        SessionInputKind::Url => true,
        SessionInputKind::Id => false,
    };

    if treat_as_url {
        let url = parsed_url
            .ok_or_else(|| "--session-url must be a full HTTPS conversation URL".to_string())?;
        let session_provider =
            resolve_session_url_provider(selected_provider, provider_was_explicit, &url)?;
        return Ok((session_provider, url.to_string()));
    }

    if !valid_session_id(session) {
        return Err(
            "Session ID may contain only ASCII letters, digits, hyphens, and underscores"
                .to_string(),
        );
    }

    if !selected_provider.capabilities().session.supports_id() {
        return Err(format!(
            "session: {} does not support raw session IDs for {}.",
            selected_provider.display_name(),
            input.kind.flag_name()
        ));
    }

    let session_url = selected_provider
        .conversation_url_from_id(session)
        .ok_or_else(|| {
            format!(
                "session: {} does not support raw session IDs in this ask-bridge version.",
                selected_provider.display_name()
            )
        })?;

    Ok((selected_provider, session_url))
}

#[derive(Clone, Copy, Debug)]
struct PageLoginState {
    id: usize,
    selected: bool,
    login_state: LoginState,
}

#[derive(Default)]
struct ResponseCompletionTracker {
    stable_done_checks: usize,
    last_response_signature: Option<(u64, u64)>,
}

impl ResponseCompletionTracker {
    fn observe(
        &mut self,
        status: &str,
        is_new: bool,
        content_length: u64,
        content_hash: u64,
        requires_text_stability: bool,
    ) -> bool {
        if status != "done" || !is_new {
            self.stable_done_checks = 0;
            if requires_text_stability {
                self.last_response_signature = None;
            }
            return false;
        }

        if requires_text_stability {
            if content_length == 0 {
                self.stable_done_checks = 0;
                self.last_response_signature = None;
                return false;
            }

            let signature = (content_length, content_hash);
            if self.last_response_signature == Some(signature) {
                self.stable_done_checks += 1;
            } else {
                self.stable_done_checks = 1;
                self.last_response_signature = Some(signature);
            }
        } else {
            self.stable_done_checks += 1;
        }

        self.stable_done_checks >= 3
    }
}

fn preferred_provider_page_id(pages: &[PageLoginState]) -> Option<usize> {
    pages
        .iter()
        .find(|page| page.login_state == LoginState::LoggedIn)
        .or_else(|| pages.iter().find(|page| page.selected))
        .or_else(|| pages.first())
        .map(|page| page.id)
}

fn parse_node_version(output: &str) -> Option<(u64, u64, u64)> {
    let version = output.trim().strip_prefix('v').unwrap_or(output.trim());
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;

    if parts.next().is_some() {
        return None;
    }

    Some((major, minor, patch))
}

fn validate_node_version_output(output: &str) -> Result<(), String> {
    let version = parse_node_version(output).ok_or_else(|| {
        format!(
            "Could not parse Node.js version from '{}'. Install a current Node.js LTS release and retry.",
            output.trim()
        )
    })?;
    let (major, minor, patch) = version;
    let supported = (major == 20 && (minor, patch) >= (19, 0))
        || (major == 22 && (minor, patch) >= (12, 0))
        || major >= 23;

    if supported {
        return Ok(());
    }

    Err(format!(
        "Node.js v{major}.{minor}.{patch} is not supported by {MCP_PACKAGE_SPEC}. Supported versions are ^20.19.0, ^22.12.0, or >=23.0.0. Install a current Node.js LTS release, reopen the terminal, and retry."
    ))
}

fn check_node_runtime() -> Result<(), String> {
    let output = Command::new("node")
        .arg("--version")
        .output()
        .map_err(|e| {
            format!(
                "Failed to run 'node --version': {e}. Install Node.js and ensure it is available in PATH."
            )
        })?;

    if !output.status.success() {
        return Err(format!(
            "'node --version' exited with status {}. Install a current Node.js LTS release and retry.",
            output.status
        ));
    }

    validate_node_version_output(&String::from_utf8_lossy(&output.stdout))
}

/// Pinned chrome-devtools-mcp package spec. `@latest` would make every npx
/// spawn re-resolve the dist-tag against the npm registry, which was observed
/// stalling; with mcp-cli's timeout-less request wait that hung whole runs
/// (2026-07-11). Bump this version deliberately and re-run the e2e check.
const MCP_PACKAGE_SPEC: &str = "chrome-devtools-mcp@1.5.0";

fn build_chrome_devtools_server_config(
    quiet_mcp: bool,
    headless: bool,
    log_path: &str,
    is_windows: bool,
) -> Value {
    let mut mcp_args = vec![
        "-y".to_string(),
        MCP_PACKAGE_SPEC.to_string(),
        "--browser-url=http://127.0.0.1:9223".to_string(),
    ];
    if quiet_mcp {
        mcp_args.push("--no-usage-statistics".to_string());
        mcp_args.push("--no-performance-crux".to_string());
    }
    if headless {
        mcp_args.push("--headless".to_string());
    }
    mcp_args.push("--logFile".to_string());
    mcp_args.push(log_path.to_string());

    let mut chrome_devtools_server = serde_json::json!({
        "command": if is_windows { "npx.cmd" } else { "npx" },
        "args": mcp_args
    });

    if quiet_mcp {
        chrome_devtools_server["env"] = serde_json::json!({
            "NPM_CONFIG_LOGLEVEL": "error",
            "NPM_CONFIG_PROGRESS": "false",
            "NPM_CONFIG_FUND": "false",
            "NPM_CONFIG_AUDIT": "false",
            "NPM_CONFIG_FUNDING": "0",
            "NPM_CONFIG_UPDATE_NOTIFIER": "false",
            "NO_COLOR": "1",
            "CI": "1",
            "NODE_NO_WARNINGS": "1"
        });
    }

    chrome_devtools_server
}

fn write_mcp_config(quiet_mcp: bool, headless: bool) -> Result<String, String> {
    let mut config_dir = home::home_dir().ok_or("Could not locate home directory")?;
    config_dir.push(".config/ask-bridge");
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("Failed to create config directory: {}", e))?;

    let log_path = config_dir
        .join("chrome-devtools-mcp.log")
        .to_string_lossy()
        .to_string();

    config_dir.push("mcp_servers.json");
    let config_path = config_dir.to_string_lossy().to_string();

    let chrome_devtools_server = build_chrome_devtools_server_config(
        quiet_mcp,
        headless,
        &log_path,
        cfg!(target_os = "windows"),
    );

    let config_content = serde_json::json!({
        "mcpServers": {
            "chrome-devtools": chrome_devtools_server
        }
    });

    let content_str = serde_json::to_string_pretty(&config_content).map_err(|e| e.to_string())?;

    std::fs::write(&config_path, content_str)
        .map_err(|e| format!("Failed to write mcp_servers.json: {}", e))?;

    Ok(config_path)
}

fn chrome_profile_path() -> Result<String, String> {
    let mut profile_dir = home::home_dir().ok_or("Could not locate home directory")?;
    profile_dir.push(".config/ask-bridge/chrome-profile");
    std::fs::create_dir_all(&profile_dir)
        .map_err(|e| format!("Failed to create chrome profile directory: {}", e))?;

    Ok(profile_dir.to_string_lossy().to_string())
}

fn chrome_pid_path() -> Result<PathBuf, String> {
    let mut path = home::home_dir().ok_or("Could not locate home directory")?;
    path.push(".config/ask-bridge/chrome.pid");
    Ok(path)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct ChromeProcessRecord {
    pid: u32,
    #[serde(default)]
    browser_id: Option<String>,
}

fn parse_chrome_process_record(content: &str) -> Option<ChromeProcessRecord> {
    serde_json::from_str(content).ok().or_else(|| {
        content
            .trim()
            .parse::<u32>()
            .ok()
            .map(|pid| ChromeProcessRecord {
                pid,
                browser_id: None,
            })
    })
}

fn write_chrome_process_record(record: &ChromeProcessRecord) -> Result<(), String> {
    let path = chrome_pid_path()?;
    let content = serde_json::to_string(record)
        .map_err(|e| format!("Failed to serialize Chrome process record: {}", e))?;
    std::fs::write(&path, content).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

fn read_chrome_process_record() -> Option<ChromeProcessRecord> {
    let path = chrome_pid_path().ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    parse_chrome_process_record(&content)
}

fn read_chrome_pid() -> Option<String> {
    read_chrome_process_record().map(|record| record.pid.to_string())
}

fn remove_chrome_pid_file() -> Result<(), String> {
    let path = chrome_pid_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Failed to remove {}: {}", path.display(), e)),
    }
}

fn browser_id_from_websocket_url(url: &str) -> Option<String> {
    const LOOPBACK_PREFIXES: &[&str] = &[
        "ws://127.0.0.1:9223/devtools/browser/",
        "ws://localhost:9223/devtools/browser/",
        "ws://[::1]:9223/devtools/browser/",
    ];
    let id = LOOPBACK_PREFIXES
        .iter()
        .find_map(|prefix| url.strip_prefix(prefix))?
        .trim();
    (!id.is_empty() && !id.contains(['/', '?', '#'])).then(|| id.to_string())
}

fn browser_id_from_version_response(response: &str) -> Option<String> {
    if !http_response_is_complete(response.as_bytes()) {
        return None;
    }
    let (headers, body) = response.split_once("\r\n\r\n")?;
    let status = headers.lines().next()?;
    let mut status_parts = status.split_whitespace();
    if !status_parts.next()?.starts_with("HTTP/") || status_parts.next()? != "200" {
        return None;
    }
    let body = body.trim();
    let version: Value = serde_json::from_str(body).ok()?;
    let websocket_url = version.get("webSocketDebuggerUrl")?.as_str()?;
    browser_id_from_websocket_url(websocket_url)
}

fn http_response_is_complete(response: &[u8]) -> bool {
    let Some(header_end) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let body_start = header_end + 4;
    let Ok(headers) = std::str::from_utf8(&response[..header_end]) else {
        return false;
    };
    let content_length = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    });

    content_length
        .and_then(|content_length| body_start.checked_add(content_length))
        .map(|response_length| response.len() >= response_length)
        .unwrap_or(false)
}

fn debug_browser_id() -> Option<String> {
    const MAX_RESPONSE_SIZE: usize = 64 * 1024;
    const TOTAL_TIMEOUT: Duration = Duration::from_secs(5);

    let mut stream = TcpStream::connect("127.0.0.1:9223").ok()?;
    let timeout = Some(Duration::from_millis(500));
    stream.set_read_timeout(timeout).ok()?;
    stream.set_write_timeout(timeout).ok()?;
    stream
        .write_all(
            b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:9223\r\nConnection: close\r\n\r\n",
        )
        .ok()?;

    let mut response = Vec::new();
    let mut buffer = [0_u8; 4096];
    let deadline = Instant::now() + TOTAL_TIMEOUT;
    loop {
        if Instant::now() >= deadline {
            break;
        }
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes_read) => {
                response
                    .len()
                    .checked_add(bytes_read)
                    .filter(|length| *length <= MAX_RESPONSE_SIZE)
                    .map(|_| ())?;
                response.extend_from_slice(&buffer[..bytes_read]);
                if http_response_is_complete(&response) {
                    break;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(_) => return None,
        }
    }

    if !http_response_is_complete(&response) {
        return None;
    }
    let response = String::from_utf8(response).ok()?;
    browser_id_from_version_response(&response)
}

fn build_chrome_process_record(
    listener_pids: &[String],
    browser_id: Option<&str>,
) -> Option<ChromeProcessRecord> {
    if listener_pids.len() != 1 {
        return None;
    }
    Some(ChromeProcessRecord {
        pid: listener_pids.first()?.parse::<u32>().ok()?,
        browser_id: Some(browser_id?.to_string()),
    })
}

#[cfg(any(target_os = "linux", test))]
const LINUX_CHROME_COMMANDS: &[&str] = &["google-chrome", "google-chrome-stable"];

#[cfg(any(target_os = "linux", test))]
fn first_existing_path(paths: &[&str]) -> Option<String> {
    paths
        .iter()
        .find(|path| Path::new(path).exists())
        .map(|path| (*path).to_string())
}

#[cfg(any(target_os = "linux", test))]
fn find_command_in_path(command: &str, path_env: Option<&std::ffi::OsStr>) -> Option<String> {
    let path_env = path_env?;

    std::env::split_paths(path_env)
        .map(|dir| dir.join(command))
        .find(|path| path.exists())
        .map(|path| path.to_string_lossy().to_string())
}

#[cfg(any(target_os = "linux", test))]
fn find_chrome_command_in_path(path_env: Option<&std::ffi::OsStr>) -> Option<String> {
    LINUX_CHROME_COMMANDS
        .iter()
        .find_map(|command| find_command_in_path(command, path_env))
}

#[cfg(any(target_os = "linux", test))]
fn find_linux_chrome_path(
    path_env: Option<&std::ffi::OsStr>,
    path_candidates: &[&str],
) -> Option<String> {
    find_chrome_command_in_path(path_env).or_else(|| first_existing_path(path_candidates))
}

fn find_chrome_path() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        // 1. Program Files
        if let Ok(pf) = std::env::var("ProgramFiles") {
            let path = format!(r"{}\Google\Chrome\Application\chrome.exe", pf);
            if std::path::Path::new(&path).exists() {
                return Ok(path);
            }
        } else {
            let path = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
            if std::path::Path::new(path).exists() {
                return Ok(path.to_string());
            }
        }

        // 2. Program Files (x86)
        if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
            let path = format!(r"{}\Google\Chrome\Application\chrome.exe", pf86);
            if std::path::Path::new(&path).exists() {
                return Ok(path);
            }
        } else {
            let path = r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe";
            if std::path::Path::new(path).exists() {
                return Ok(path.to_string());
            }
        }

        // 3. LocalAppData
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let path = format!(r"{}\Google\Chrome\Application\chrome.exe", local_app_data);
            if std::path::Path::new(&path).exists() {
                return Ok(path);
            }
        }

        Err("Google Chrome was not found in standard Windows installation paths. Please install Google Chrome.".to_string())
    }

    #[cfg(target_os = "macos")]
    {
        let path = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        if std::path::Path::new(path).exists() {
            Ok(path.to_string())
        } else {
            Err("Google Chrome not found at /Applications/Google Chrome.app".to_string())
        }
    }

    #[cfg(target_os = "linux")]
    {
        const LINUX_CHROME_PATHS: &[&str] = &[
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/local/bin/google-chrome",
            "/usr/local/bin/google-chrome-stable",
            "/opt/google/chrome/google-chrome",
        ];

        let path_env = std::env::var_os("PATH");
        find_linux_chrome_path(path_env.as_deref(), LINUX_CHROME_PATHS).ok_or_else(|| {
            "Google Chrome was not found in PATH or standard Linux installation paths. Please install Google Chrome or add google-chrome to PATH.".to_string()
        })
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err("Google Chrome auto-detection is not supported on this operating system. Please use macOS, Windows, or Linux.".to_string())
    }
}

fn start_chrome_if_needed(headless: bool, verbose: bool) -> Result<(), String> {
    let profile_path = chrome_profile_path()?;

    if TcpStream::connect("127.0.0.1:9223").is_ok() {
        let snapshot = inspect_chrome_debug_port(&profile_path);
        if debug_listener_scope_is_unambiguous(&snapshot.listener_pids)
            && chrome_record_matches_current(
                snapshot.record.as_ref(),
                snapshot.browser_id.as_deref(),
                &snapshot.listener_pids,
            )
        {
            if headless {
                // Force hide any existing background Chrome PIDs asynchronously just in case they are currently visible
                #[cfg(target_os = "macos")]
                {
                    let pids = snapshot.ask_pids.clone();
                    thread::spawn(move || {
                        for pid_str in pids {
                            if let Ok(pid) = pid_str.parse::<u32>() {
                                let script = format!(
                                    "tell application \"System Events\" to set visible of first application process whose unix id is {} to false",
                                    pid
                                );
                                let _ = Command::new("osascript").arg("-e").arg(&script).status();
                            }
                        }
                    });
                }
            }
            if verbose && headless && !is_debug_chrome_background(&profile_path) {
                println!(
                    "Reusing existing ask-bridge Chrome on port 9223. Run `ask-bridge close` if you want to restart it in background mode."
                );
            }
            return Ok(());
        }

        if debug_listener_scope_is_unambiguous(&snapshot.listener_pids)
            && !snapshot.ask_pids.is_empty()
            && build_chrome_process_record(&snapshot.listener_pids, snapshot.browser_id.as_deref())
                .is_some()
        {
            if let Some(record) =
                build_chrome_process_record(&snapshot.listener_pids, snapshot.browser_id.as_deref())
            {
                write_chrome_process_record(&record).map_err(|error| {
                    format!("Failed to update Chrome process record: {}", error)
                })?;
            }
            if verbose {
                println!("Reusing the existing ask-bridge Chrome on port 9223.");
            }
            return Ok(());
        }

        return Err(
            "Port 9223 is already used by a non-ask Chrome process. Stop it or use a different debugging port."
                .to_string(),
        );
    }

    if verbose {
        println!(
            "Chrome is not running on port 9223. Starting Chrome with remote debugging (headless: {})...",
            headless
        );
    }

    let chrome_path = find_chrome_path()?;
    let _ = remove_chrome_pid_file();

    let mut cmd = Command::new(&chrome_path);
    cmd.arg("--remote-debugging-port=9223")
        .arg(format!("--user-data-dir={}", profile_path))
        .arg(ASK_BRIDGE_CHROME_MARKER)
        .arg("--no-first-run")
        .arg("--no-default-browser-check");

    #[cfg(target_os = "windows")]
    {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    if headless {
        cmd.arg("--ask-bridge-background")
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--window-size=1440,1200")
            .arg("--window-position=-2000,-2000");
    }

    let child = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to start Google Chrome: {}", e))?;

    let child_pid = child.id();

    if verbose {
        println!(
            "Started ask-bridge Chrome PID {} with profile {}.",
            child_pid, profile_path
        );
    }

    if headless {
        #[cfg(target_os = "macos")]
        {
            let pid = child.id();
            thread::spawn(move || {
                // Rapidly set visibility to false during startup to prevent window from flashing or drawing
                for _ in 0..40 {
                    let script = format!(
                        "tell application \"System Events\" to try\nset visible of first application process whose unix id is {} to false\nend try",
                        pid
                    );
                    let _ = Command::new("osascript").arg("-e").arg(&script).status();
                    thread::sleep(Duration::from_millis(50));
                }
            });
        }
    }

    let _ = child; // Avoid unused variable warning on non-macOS platforms

    // Wait for Chrome to listen and prove that the listener belongs to this launch.
    let startup_deadline = Instant::now() + Duration::from_secs(15);
    let mut last_identity_error = None;
    while Instant::now() < startup_deadline {
        if TcpStream::connect("127.0.0.1:9223").is_ok() {
            let snapshot = inspect_chrome_debug_port(&profile_path);
            if let Some(record) =
                build_chrome_process_record(&snapshot.listener_pids, snapshot.browser_id.as_deref())
            {
                if let Err(error) = write_chrome_process_record(&record) {
                    return Err(format!(
                        "Failed to record Chrome process identity: {}",
                        error
                    ));
                }
                if verbose && record.pid != child_pid {
                    println!(
                        "Recorded actual Chrome listener PID {} (launcher PID {}).",
                        record.pid, child_pid
                    );
                }
                if verbose {
                    println!("Chrome started and listening on port 9223.");
                }
                return Ok(());
            }
            last_identity_error = Some(
                "Chrome did not expose a valid CDP browser identity on port 9223.".to_string(),
            );
        }
        thread::sleep(Duration::from_millis(100));
    }

    let _ = remove_chrome_pid_file();
    match last_identity_error {
        Some(error) => Err(format!(
            "Failed to identify active Chrome listener: {}",
            error
        )),
        None => Err("Timed out waiting for Chrome to start on port 9223".to_string()),
    }
}

fn normalize_profile_match_text(value: &str) -> String {
    let normalized = value.replace('\\', "/").replace(['"', '\''], "");

    #[cfg(target_os = "windows")]
    {
        normalized.to_ascii_lowercase()
    }

    #[cfg(not(target_os = "windows"))]
    {
        normalized
    }
}

fn command_has_argument(command: &str, argument: &str) -> bool {
    command.match_indices(argument).any(|(start, matched)| {
        let before_is_boundary = start == 0
            || command[..start]
                .chars()
                .next_back()
                .map(char::is_whitespace)
                .unwrap_or(false);
        let end = start + matched.len();
        let after_is_boundary = end == command.len()
            || command[end..]
                .chars()
                .next()
                .map(char::is_whitespace)
                .unwrap_or(false);
        before_is_boundary && after_is_boundary
    })
}

fn command_uses_profile(command: &str, profile_path: &str) -> bool {
    let command = normalize_profile_match_text(command);
    let profile_path = normalize_profile_match_text(profile_path);

    command_has_argument(&command, &format!("--user-data-dir={}", profile_path))
        || command_has_argument(&command, &format!("--user-data-dir {}", profile_path))
}

fn command_identifies_ask_chrome(command: &str, profile_path: &str) -> bool {
    command_uses_profile(command, profile_path)
        || command_has_argument(command, ASK_BRIDGE_CHROME_MARKER)
}

fn find_ask_chrome_owner_pid_with<C, P>(
    listener_pid: &str,
    profile_path: &str,
    mut command_for: C,
    mut parent_for: P,
) -> Option<String>
where
    C: FnMut(&str) -> Option<String>,
    P: FnMut(&str) -> Option<String>,
{
    let mut current_pid = listener_pid.to_string();

    for _ in 0..16 {
        if command_for(&current_pid)
            .map(|command| command_identifies_ask_chrome(&command, profile_path))
            .unwrap_or(false)
        {
            return Some(current_pid);
        }

        let parent_pid = parent_for(&current_pid)?;
        if parent_pid.is_empty() || parent_pid == "0" || parent_pid == current_pid {
            return None;
        }
        current_pid = parent_pid;
    }

    None
}

fn chrome_record_matches_browser(record: &ChromeProcessRecord, browser_id: Option<&str>) -> bool {
    matches!(
        (record.browser_id.as_deref(), browser_id),
        (Some(recorded_id), Some(current_id)) if recorded_id == current_id
    )
}

fn chrome_record_matches_current(
    record: Option<&ChromeProcessRecord>,
    browser_id: Option<&str>,
    listener_pids: &[String],
) -> bool {
    record.is_some_and(|record| chrome_record_matches_browser(record, browser_id))
        && listener_pids.len() == 1
}

fn find_ask_chrome_owner_pids_with<C, P>(
    listener_pids: &[String],
    profile_path: &str,
    mut command_for: C,
    mut parent_for: P,
) -> Vec<String>
where
    C: FnMut(&str) -> Option<String>,
    P: FnMut(&str) -> Option<String>,
{
    let mut ask_pids = Vec::new();
    for listener_pid in listener_pids {
        let ask_pid = find_ask_chrome_owner_pid_with(
            listener_pid,
            profile_path,
            &mut command_for,
            &mut parent_for,
        );

        if let Some(ask_pid) = ask_pid
            && !ask_pids.contains(&ask_pid)
        {
            ask_pids.push(ask_pid);
        }
    }
    ask_pids
}

struct ChromeDebugSnapshot {
    listener_pids: Vec<String>,
    record: Option<ChromeProcessRecord>,
    browser_id: Option<String>,
    ask_pids: Vec<String>,
}

fn debug_listener_scope_is_unambiguous(listener_pids: &[String]) -> bool {
    listener_pids.len() <= 1
}

fn inspect_chrome_debug_port(profile_path: &str) -> ChromeDebugSnapshot {
    let listener_pids = debug_port_listener_pids();
    let record = read_chrome_process_record();
    let browser_id = debug_browser_id();
    let ask_pids = find_ask_chrome_owner_pids_with(
        &listener_pids,
        profile_path,
        process_command,
        process_parent_pid,
    );
    ChromeDebugSnapshot {
        listener_pids,
        record,
        browser_id,
        ask_pids,
    }
}

fn ask_chrome_pids_on_debug_port(profile_path: &str) -> Vec<String> {
    inspect_chrome_debug_port(profile_path).ask_pids
}

#[cfg(target_os = "windows")]
fn parse_windows_netstat_listener_pids(output: &str, port: u16) -> Vec<String> {
    let mut pids = Vec::new();
    for line in output.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5
            || !fields[0].eq_ignore_ascii_case("TCP")
            || !fields[3].eq_ignore_ascii_case("LISTENING")
            || fields[1]
                .rsplit_once(':')
                .and_then(|(_, port)| port.parse::<u16>().ok())
                != Some(port)
        {
            continue;
        }

        let pid = fields[4];
        if pid.chars().all(|character| character.is_ascii_digit())
            && !pids.iter().any(|existing| existing == pid)
        {
            pids.push(pid.to_string());
        }
    }
    pids
}

fn debug_port_listener_pids() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("netstat").args(["-ano", "-p", "tcp"]).output();

        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                parse_windows_netstat_listener_pids(&stdout, 9223)
            }
            _ => Vec::new(),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let output = Command::new("lsof")
            .args(["-tiTCP:9223", "-sTCP:LISTEN"])
            .output();

        match output {
            Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(target_os = "windows")]
fn parse_wmic_column_value(output: &str) -> Option<String> {
    let mut non_empty_lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    non_empty_lines.next()?;
    non_empty_lines.next().map(str::to_string)
}

fn process_command(pid: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("wmic")
            .args([
                "process",
                "where",
                &format!("processid={}", pid),
                "get",
                "commandline",
            ])
            .output();

        if let Ok(out) = output
            && out.status.success()
        {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if let Some(command) = parse_wmic_column_value(&stdout) {
                return Some(command);
            }
        }

        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "(Get-CimInstance Win32_Process -Filter 'ProcessId = {}').CommandLine",
                    pid
                ),
            ])
            .output();

        if let Ok(out) = output
            && out.status.success()
        {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !stdout.is_empty() {
                return Some(stdout);
            }
        }

        None
    }

    #[cfg(not(target_os = "windows"))]
    {
        let output = Command::new("ps")
            .args(["-p", pid, "-o", "command="])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

fn process_parent_pid(pid: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("wmic")
            .args([
                "process",
                "where",
                &format!("processid={}", pid),
                "get",
                "parentprocessid",
            ])
            .output();

        if let Ok(out) = output
            && out.status.success()
            && let Some(parent_pid) = parse_wmic_column_value(&String::from_utf8_lossy(&out.stdout))
        {
            return Some(parent_pid);
        }

        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "(Get-CimInstance Win32_Process -Filter 'ProcessId = {}').ParentProcessId",
                    pid
                ),
            ])
            .output();

        if let Ok(out) = output
            && out.status.success()
        {
            let parent_pid = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !parent_pid.is_empty() {
                return Some(parent_pid);
            }
        }

        None
    }

    #[cfg(not(target_os = "windows"))]
    {
        let output = Command::new("ps")
            .args(["-p", pid, "-o", "ppid="])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let parent_pid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if parent_pid.is_empty() {
            None
        } else {
            Some(parent_pid)
        }
    }
}

fn is_debug_chrome_background(profile_path: &str) -> bool {
    ask_chrome_pids_on_debug_port(profile_path)
        .iter()
        .any(|pid| {
            process_command(pid)
                .map(|cmd| cmd.contains("--ask-bridge-background"))
                .unwrap_or(false)
        })
}

fn wait_for_debug_port_to_close() -> bool {
    for _ in 0..50 {
        if TcpStream::connect("127.0.0.1:9223").is_err() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn close_ask_chrome_on_debug_port(profile_path: &str) -> Result<bool, String> {
    let snapshot = inspect_chrome_debug_port(profile_path);
    if snapshot.listener_pids.is_empty() {
        if TcpStream::connect("127.0.0.1:9223").is_ok() {
            return Err(
                "Port 9223 is active, but ask-bridge could not identify its listener process. No process was closed."
                    .to_string(),
            );
        }
        if let Err(_error) = remove_chrome_pid_file() {
            // ignore cleanup failure when port is already closed
        }
        return Ok(false);
    }
    if !debug_listener_scope_is_unambiguous(&snapshot.listener_pids) {
        return Err(
            "Multiple processes are listening on port 9223, so ask-bridge cannot safely determine which process to close. No process was closed."
                .to_string(),
        );
    }

    if snapshot.ask_pids.is_empty() {
        return Err(
            "Port 9223 is already used by a non-ask Chrome process. Stop it or use a different debugging port."
                .to_string(),
        );
    }

    for pid in &snapshot.ask_pids {
        #[cfg(target_os = "windows")]
        {
            let _ = Command::new("taskkill")
                .args(["/PID", pid, "/T"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = Command::new("kill").args(["-TERM", pid]).status();
        }
    }

    if wait_for_debug_port_to_close() {
        let _ = remove_chrome_pid_file();
        return Ok(true);
    }

    #[cfg(target_os = "windows")]
    {
        let current = inspect_chrome_debug_port(profile_path);
        if current.listener_pids != snapshot.listener_pids
            || current.browser_id != snapshot.browser_id
            || current.ask_pids.is_empty()
        {
            return Err(
                "The Chrome process on port 9223 changed while closing it; force termination was cancelled."
                    .to_string(),
            );
        }

        for pid in &current.ask_pids {
            let _ = Command::new("taskkill")
                .args(["/F", "/PID", pid, "/T"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }

        if wait_for_debug_port_to_close() {
            let _ = remove_chrome_pid_file();
            return Ok(true);
        }
    }

    Err("Timed out waiting for existing ask-bridge Chrome to stop".to_string())
}

static FORWARD_MCP_STDERR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// One MCP session per run: a single long-lived chrome-devtools-mcp child plus
/// the tokio runtime that drives its background reader tasks.
///
/// Upstream called `McpClient::call_tool` per browser action, which spawns a
/// fresh `npx chrome-devtools-mcp` child for every single action (~50 per
/// query) and waits on its response without any timeout — one stalled npx
/// spawn hung the whole run forever (2026-07-11). Reusing one connection
/// removes the re-spawn churn; `MCP_CALL_TIMEOUT` turns any remaining stall
/// into a loud, bounded error (see `mcp_error_is_transport` for why the failed
/// call is not replayed).
struct McpSession {
    connection: McpConnection,
    runtime: tokio::runtime::Runtime,
    config_path: String,
}

static MCP_SESSION: std::sync::Mutex<Option<McpSession>> = std::sync::Mutex::new(None);

const MCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(120);
const MCP_CALL_TIMEOUT: Duration = Duration::from_secs(90);
const MCP_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

fn mcp_session_connect(config_path: &str) -> Result<McpSession, String> {
    let client = McpClient::load(Some(config_path))
        .map_err(|e| format!("Failed to load MCP config: {}", e))?;
    let server_config = client
        .server_config("chrome-devtools")
        .map_err(|e| format!("Missing chrome-devtools MCP server config: {}", e))?;
    // A multi-thread runtime with one worker keeps the connection's background
    // stdout/stderr reader tasks running between calls (a current-thread
    // runtime only makes progress inside block_on).
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| format!("Failed to create async runtime for MCP session: {}", e))?;
    let connection = runtime.block_on(async {
        // Connect the stdio transport directly: mcp-cli's default path first
        // tries its persistent daemon, which re-execs this binary with
        // `--daemon` — an entrypoint ask-bridge does not implement — so that
        // path can only ever fail and fall back.
        let connect_future = async {
            match &server_config {
                ServerConfig::Stdio(stdio_config) => {
                    StdioClient::connect("chrome-devtools", stdio_config)
                        .await
                        .map(McpConnection::Stdio)
                }
                _ => client.connect("chrome-devtools").await,
            }
        };
        match tokio::time::timeout(MCP_CONNECT_TIMEOUT, connect_future).await {
            Err(_) => Err(format!(
                "Failed to start chrome-devtools MCP server: timed out after {}s",
                MCP_CONNECT_TIMEOUT.as_secs()
            )),
            Ok(result) => {
                result.map_err(|e| format!("Failed to start chrome-devtools MCP server: {}", e))
            }
        }
    })?;
    Ok(McpSession {
        connection,
        runtime,
        config_path: config_path.to_string(),
    })
}

fn mcp_session_reset(slot: &mut Option<McpSession>) {
    if let Some(session) = slot.take() {
        let McpSession {
            connection,
            runtime,
            ..
        } = session;
        // Best-effort close (kills the child); if even that stalls, dropping
        // the runtime stops the background tasks and the orphaned child exits
        // on stdin EOF.
        let _ = runtime
            .block_on(async { tokio::time::timeout(MCP_CLOSE_TIMEOUT, connection.close()).await });
    }
}

fn mcp_session_call(
    slot: &mut Option<McpSession>,
    config_path: &str,
    tool: &str,
    args: Value,
) -> Result<Value, String> {
    let needs_connect = slot
        .as_ref()
        .map(|session| session.config_path != config_path)
        .unwrap_or(true);
    if needs_connect {
        mcp_session_reset(slot);
        *slot = Some(mcp_session_connect(config_path)?);
    }
    let session = slot.as_ref().expect("session connected above");
    session.runtime.block_on(async {
        match tokio::time::timeout(MCP_CALL_TIMEOUT, session.connection.call_tool(tool, args)).await
        {
            Err(_) => Err(format!(
                "MCP tool '{}' timed out after {}s",
                tool,
                MCP_CALL_TIMEOUT.as_secs()
            )),
            Ok(result) => result.map_err(|e| format!("mcp-cli library call failed: {}", e)),
        }
    })
}

/// Errors that mean the MCP transport itself is dead or wedged: our own
/// timeouts, or transport-level failures (dead child / closed pipes — exact
/// phrases from mcp-cli's StdioClient). These earn a session reset so the next
/// command starts clean. The failed call is deliberately NOT replayed: a
/// timed-out request may already have executed in the browser (replaying a
/// submit would double-post), and a fresh chrome-devtools-mcp child forgets
/// the selected page (a replay could act on the wrong tab). Application-level
/// tool errors (e.g. a JS exception from evaluate_script) propagate unchanged.
fn mcp_error_is_transport(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("timed out")
        || lower.contains("failed to send request to process stdin")
        || lower.contains("server process exited unexpectedly")
        || lower.contains("stdio response receiver canceled")
        || lower.contains("failed to start chrome-devtools mcp server")
}

fn call_mcp_tool(config_path: &str, tool: &str, args: Value) -> Result<Value, String> {
    let _stderr_guard = if FORWARD_MCP_STDERR.load(std::sync::atomic::Ordering::Relaxed) {
        None
    } else {
        Some(
            gag::Gag::stderr()
                .map_err(|e| format!("Failed to suppress MCP stderr in quiet mode: {}", e))?,
        )
    };

    let mut slot = MCP_SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match mcp_session_call(&mut slot, config_path, tool, args) {
        Ok(value) => Ok(value),
        Err(error) => {
            if mcp_error_is_transport(&error) {
                mcp_session_reset(&mut slot);
                return Err(format!(
                    "{} (MCP session was reset; re-run the command)",
                    error
                ));
            }
            Err(error)
        }
    }
}

fn parse_pages(text: &str) -> Vec<Page> {
    let mut pages = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("##") {
            continue;
        }
        if let Some((id_str, rest)) = line.split_once(':') {
            let id = match id_str.trim().parse::<usize>() {
                Ok(id) => id,
                Err(_) => continue,
            };
            let rest = rest.trim();
            let (url, selected) = if rest.ends_with("[selected]") {
                let url = rest.strip_suffix("[selected]").unwrap().trim().to_string();
                (url, true)
            } else {
                (rest.to_string(), false)
            };
            pages.push(Page { id, url, selected });
        }
    }
    pages
}

fn parse_script_result(val: &Value) -> Result<Value, String> {
    let text = val
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|obj| obj.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| "Could not extract text field from evaluate_script result".to_string())?;

    let start_tag = "```json";

    if let Some(start_pos) = text.find(start_tag) {
        let json_start = start_pos + start_tag.len();
        let json_str = text[json_start..].trim_start();
        let mut values = serde_json::Deserializer::from_str(json_str).into_iter::<Value>();
        let parsed = values
            .next()
            .ok_or_else(|| "JSON parsing error: missing JSON value".to_string())?
            .map_err(|e| format!("JSON parsing error: {}", e))?;
        let remainder = json_str[values.byte_offset()..].trim_start();
        let after_fence = remainder
            .strip_prefix("```")
            .ok_or_else(|| "Could not find closing JSON fence in script result".to_string())?;
        if !matches!(after_fence.chars().next(), None | Some('\r') | Some('\n')) {
            return Err("Invalid closing JSON fence in script result".to_string());
        }
        return Ok(parsed);
    }

    Err("Could not find JSON fencing in script result".to_string())
}

fn tool_text(val: &Value) -> Result<String, String> {
    val.get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|obj| obj.get("text"))
        .and_then(|t| t.as_str())
        .map(|text| text.to_string())
        .ok_or_else(|| format!("Could not extract text field from tool result: {:?}", val))
}

fn take_snapshot_text(config_path: &str) -> Result<String, String> {
    let res = call_mcp_tool(config_path, "take_snapshot", serde_json::json!({}))?;
    tool_text(&res)
}

fn extract_snapshot_uid(line: &str) -> Option<String> {
    let marker_pos = line.find("uid=")?;
    let mut rest = line[marker_pos + 4..].trim_start();
    rest = rest.trim_start_matches(['"', '\'', '[']);
    let uid: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'' && *c != ']')
        .collect();
    if uid.is_empty() { None } else { Some(uid) }
}

fn find_snapshot_uid(snapshot: &str, include: &[&str], exclude: &[&str]) -> Option<String> {
    snapshot.lines().find_map(|line| {
        let lower = line.to_lowercase();
        let includes_all = include
            .iter()
            .all(|needle| lower.contains(&needle.to_lowercase()));
        let excludes_all = exclude
            .iter()
            .all(|needle| !lower.contains(&needle.to_lowercase()));
        if includes_all && excludes_all {
            extract_snapshot_uid(line)
        } else {
            None
        }
    })
}

fn find_m365_composer_uid(snapshot: &str) -> Option<String> {
    find_snapshot_uid(
        snapshot,
        &["textbox", "copilot"],
        &["search", "搜尋", "搜索"],
    )
}

fn is_glow_available() -> bool {
    Command::new("glow")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn render_markdown(markdown: &str, use_glow: bool) -> Result<(), String> {
    if markdown.is_empty() {
        return Ok(());
    }

    if use_glow {
        let glow = Command::new("glow")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn();

        if let Ok(mut child) = glow {
            let stdin_opt = child.stdin.take();
            if let Some(mut stdin) = stdin_opt {
                let _ = stdin.write_all(markdown.as_bytes()).map_err(|e| {
                    eprintln!("Failed to send Markdown content to glow: {}", e);
                });
            }

            match child.wait() {
                Ok(status) if status.success() => {
                    return Ok(());
                }
                Ok(status) => {
                    eprintln!("glow exited with status: {}", status);
                }
                Err(e) => {
                    eprintln!("Failed to wait for glow process: {}", e);
                }
            }
        }
    }

    print!("{}", markdown);
    io::stdout()
        .flush()
        .map_err(|e| format!("Failed to flush stdout: {}", e))?;

    Ok(())
}

fn requested_m365_v2_flag(cli: &Cli) -> Option<&'static str> {
    session_input(cli)
        .map(|input| input.kind.flag_name())
        .or_else(|| (!cli.images.is_empty()).then_some("--image"))
        .or_else(|| (!cli.files.is_empty()).then_some("--file"))
        .or_else(|| cli.model.is_some().then_some("--model"))
        .or_else(|| cli.reasoning.is_some().then_some("--reasoning"))
        .or_else(|| cli.image_output.is_some().then_some("--image-output"))
}

fn validate_provider_feature_support_for_platform(
    provider: Provider,
    cli: &Cli,
    is_windows: bool,
) -> Result<(), String> {
    let session = session_input(cli);
    if session.is_some() && cli.command.is_some() {
        return Err(
            "Session options are supported only for a prompt invocation, not with a subcommand"
                .to_string(),
        );
    }

    if provider == Provider::M365Copilot
        && !is_windows
        && let Some(flag) = requested_m365_v2_flag(cli)
    {
        return Err(format!(
            "Microsoft 365 Copilot V2 {flag} is Windows-only experimental in this ask-bridge version."
        ));
    }

    let capabilities = provider.capabilities_for_platform(is_windows);
    if let Some(input) = session {
        let input_is_url = input.kind == SessionInputKind::Url
            || (input.kind == SessionInputKind::Auto && Url::parse(input.value.trim()).is_ok());
        if input_is_url && !capabilities.session.supports_url() {
            return Err(format!(
                "session: {} does not support conversation URLs for {} in this ask-bridge version.",
                provider.display_name(),
                input.kind.flag_name()
            ));
        }
        if !input_is_url && !capabilities.session.supports_id() {
            return Err(format!(
                "session: {} does not support raw session IDs for {} in this ask-bridge version.",
                provider.display_name(),
                input.kind.flag_name()
            ));
        }
    }
    if !cli.images.is_empty() && !capabilities.images {
        if provider == Provider::Gemini {
            return Err(
                "Gemini image attachments are not supported yet. Use --file for Gemini document attachments."
                    .to_string(),
            );
        }
        return Err(format!(
            "{} does not support --image in this ask-bridge version.",
            provider.display_name()
        ));
    }
    if !cli.files.is_empty() && !capabilities.files {
        return Err(format!(
            "{} does not support --file in this ask-bridge version.",
            provider.display_name()
        ));
    }
    if cli.model.is_some() && !capabilities.model_selection {
        return Err(format!(
            "{} does not support --model in this ask-bridge version.",
            provider.display_name()
        ));
    }
    if cli.reasoning.is_some() && !capabilities.reasoning {
        return Err(format!(
            "{} does not support --reasoning in this ask-bridge version.",
            provider.display_name()
        ));
    }
    if cli.image_output.is_some() && !capabilities.image_download {
        return Err(format!(
            "{} does not support --image-output in this ask-bridge version.",
            provider.display_name()
        ));
    }

    Ok(())
}

fn validate_provider_feature_support(provider: Provider, cli: &Cli) -> Result<(), String> {
    validate_provider_feature_support_for_platform(provider, cli, cfg!(target_os = "windows"))
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    fn session(kind: SessionInputKind, value: &str) -> SessionInput<'_> {
        SessionInput { kind, value }
    }

    #[test]
    fn validates_chrome_devtools_mcp_node_versions() {
        for version in [
            "v20.19.0",
            "v20.20.1\r\n",
            "v22.12.0",
            "v22.15.1",
            "v23.0.0",
            "v24.4.1",
        ] {
            assert!(
                validate_node_version_output(version).is_ok(),
                "expected {version:?} to be supported"
            );
        }

        for version in ["v18.20.8", "v20.17.0", "v20.18.9", "v21.7.3", "v22.11.0"] {
            assert!(
                validate_node_version_output(version).is_err(),
                "expected {version:?} to be rejected"
            );
        }
    }

    #[test]
    fn reports_actionable_node_version_errors() {
        let unsupported = validate_node_version_output("v20.17.0").unwrap_err();
        assert!(unsupported.contains("v20.17.0"));
        assert!(unsupported.contains("^20.19.0"));
        assert!(unsupported.contains("reopen the terminal"));

        for output in ["", "20.19", "not-a-version", "v20.19.0.1"] {
            assert!(
                validate_node_version_output(output).is_err(),
                "expected {output:?} to be rejected"
            );
        }
    }

    #[test]
    fn pins_chrome_devtools_mcp_version() {
        // `@latest` makes every npx spawn re-resolve the dist-tag against the
        // npm registry; combined with mcp-cli's timeout-less request wait this
        // hung whole runs (2026-07-11). The package spec must pin a version.
        let config = build_chrome_devtools_server_config(true, true, "/tmp/mcp.log", false);
        let args = config["args"].as_array().expect("args array");
        let pkg = args
            .iter()
            .filter_map(|a| a.as_str())
            .find(|a| a.starts_with("chrome-devtools-mcp"))
            .expect("chrome-devtools-mcp package argument");
        assert!(
            !pkg.ends_with("@latest"),
            "chrome-devtools-mcp must be version-pinned, got {pkg}"
        );
        let version = pkg.rsplit('@').next().unwrap_or_default();
        assert!(
            version.chars().next().is_some_and(|c| c.is_ascii_digit()),
            "expected an explicit pinned version, got {pkg}"
        );
    }

    #[test]
    fn classifies_transport_errors_for_reconnect() {
        // Transport failures earn a session reset + loud error (exact phrases
        // from mcp-cli's StdioClient surface inside CliError's `Details:`
        // line); the call is never replayed — see mcp_error_is_transport...
        for transport in [
            "MCP tool 'click' timed out after 90s",
            "Error [SERVER_CONNECTION_FAILED]: x\n  Details: Failed to send request to process stdin",
            "Error [TOOL_EXECUTION_FAILED]: x\n  Details: Server process exited unexpectedly. Last stderr:\nnpm error",
            "Error [SERVER_CONNECTION_FAILED]: x\n  Details: Stdio response receiver canceled",
            "Failed to start chrome-devtools MCP server: timed out after 120s",
        ] {
            assert!(
                mcp_error_is_transport(transport),
                "expected transport-class error: {transport}"
            );
        }
        // ...application-level tool errors must NOT reset the session — the
        // transport is fine and the caller needs the original error.
        for app_level in [
            "mcp-cli library call failed: Error [TOOL_EXECUTION_FAILED]: Tool \"click\" execution failed\n  Details: element not found",
            "mcp-cli library call failed: Error [TOOL_EXECUTION_FAILED]: Tool \"evaluate_script\" execution failed\n  Details: TypeError: x is undefined",
        ] {
            assert!(
                !mcp_error_is_transport(app_level),
                "expected app-level error to pass through: {app_level}"
            );
        }
    }

    #[test]
    fn piped_stdin_grace_skips_silent_pipe_when_prompt_argument_present() {
        // Agent harnesses (Claude Code / Codex) run commands with a non-tty
        // stdin they may never close; blocking on EOF hung whole runs
        // (2026-07-11). With a prompt argument in hand, a silent pipe must be
        // treated as "no piped input" after the grace period.
        let (_probe_tx, probe_rx) = std::sync::mpsc::channel::<StdinProbe>();
        let (_data_tx, data_rx) = std::sync::mpsc::channel::<std::io::Result<String>>();
        let out = recv_piped_stdin(&probe_rx, &data_rx, Duration::from_millis(50), true)
            .expect("silent pipe should yield empty stdin, not an error");
        assert_eq!(out, "");
    }

    #[test]
    fn piped_stdin_reads_live_pipe_to_eof_when_prompt_argument_present() {
        // A pipe that delivers data keeps the documented combine behavior:
        // `cat notes.md | ask-bridge '摘要'` must still append stdin.
        let (probe_tx, probe_rx) = std::sync::mpsc::channel();
        let (data_tx, data_rx) = std::sync::mpsc::channel();
        probe_tx.send(StdinProbe::Data).unwrap();
        data_tx.send(Ok("piped context".to_string())).unwrap();
        let out = recv_piped_stdin(&probe_rx, &data_rx, Duration::from_millis(50), true)
            .expect("live pipe should be read");
        assert_eq!(out, "piped context");
    }

    #[test]
    fn piped_stdin_waits_unbounded_when_no_prompt_argument() {
        // Without a prompt argument stdin IS the prompt: keep upstream's
        // unbounded wait even when data arrives long after any grace window.
        let (_probe_tx, probe_rx) = std::sync::mpsc::channel();
        let (data_tx, data_rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(120));
            let _ = data_tx.send(Ok("stdin is the prompt".to_string()));
        });
        let out = recv_piped_stdin(&probe_rx, &data_rx, Duration::from_millis(10), false)
            .expect("unbounded wait should return the piped prompt");
        assert_eq!(out, "stdin is the prompt");
    }

    #[test]
    fn builds_direct_quiet_mcp_configs() {
        fn config_args(config: &serde_json::Value) -> Vec<&str> {
            config["args"]
                .as_array()
                .expect("MCP config should contain an args array")
                .iter()
                .map(|arg| arg.as_str().expect("MCP arguments should be strings"))
                .collect()
        }

        let log_path = r"C:\Temp\ask bridge\chrome-devtools-mcp.log";
        let quiet_windows = build_chrome_devtools_server_config(true, true, log_path, true);
        let verbose_windows = build_chrome_devtools_server_config(false, true, log_path, true);
        let quiet_unix = build_chrome_devtools_server_config(true, true, log_path, false);
        let quiet_args = config_args(&quiet_windows);
        let verbose_args = config_args(&verbose_windows);

        assert_eq!(quiet_windows["command"].as_str(), Some("npx.cmd"));
        assert_eq!(verbose_windows["command"].as_str(), Some("npx.cmd"));
        assert_eq!(quiet_unix["command"].as_str(), Some("npx"));
        for required in [
            MCP_PACKAGE_SPEC,
            "--browser-url=http://127.0.0.1:9223",
            "--headless",
            "--logFile",
            log_path,
        ] {
            assert!(quiet_args.contains(&required));
            assert!(verbose_args.contains(&required));
        }
        assert!(quiet_args.contains(&"--no-usage-statistics"));
        assert!(quiet_args.contains(&"--no-performance-crux"));
        assert!(!verbose_args.contains(&"--no-usage-statistics"));
        assert!(!verbose_args.contains(&"--no-performance-crux"));
        assert!(!quiet_args.iter().any(|arg| arg.contains("2>nul")));
        assert_eq!(quiet_windows["env"]["CI"].as_str(), Some("1"));
        assert!(verbose_windows.get("env").is_none());
    }

    #[test]
    fn parses_script_result_containing_markdown_code_fence() {
        let markdown = "說明\n```rust\nfn main() { println!(\"ok\"); }\n```\n結尾";
        let encoded = serde_json::to_string(markdown).expect("markdown should serialize");
        let result = serde_json::json!({
            "content": [{
                "type": "text",
                "text": format!("Script ran on page and returned:\n```json\n{}\n```", encoded)
            }]
        });

        assert_eq!(
            parse_script_result(&result).expect("script result should parse"),
            serde_json::Value::String(markdown.to_string())
        );
    }

    #[test]
    fn rejects_malformed_script_fence_without_leaking_payload() {
        let secret = "private-response-content";
        let encoded = serde_json::to_string(secret).expect("secret should serialize");

        for text in [
            format!("Script ran on page and returned:\n```json\n{}", encoded),
            format!(
                "Script ran on page and returned:\n```json\n{} trailing-data\n```",
                encoded
            ),
        ] {
            let result = serde_json::json!({
                "content": [{ "type": "text", "text": text }]
            });
            let error = parse_script_result(&result).expect_err("malformed fence should fail");

            assert!(!error.contains(secret));
        }
    }

    #[test]
    fn rejects_malformed_script_shape_without_leaking_payload() {
        let secret = "private-response-content";
        let result = serde_json::json!({
            "content": [{ "type": "text", "unexpected": secret }]
        });
        let error = parse_script_result(&result).expect_err("malformed shape should fail");

        assert!(!error.contains(secret));
        assert!(error.contains("Could not extract text field"));
    }

    fn make_test_dir(name: &str) -> std::path::PathBuf {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "ask_bridge_{}_{}_{}",
            name,
            std::process::id(),
            timestamp
        ))
    }

    #[test]
    fn parses_provider_as_global_argument() {
        let cli = Cli::try_parse_from(["ask-bridge", "--provider", "gemini", "login"]).unwrap();
        assert_eq!(cli.provider, Some(Provider::Gemini));
        assert!(matches!(cli.command, Some(Commands::Login)));

        let cli = Cli::try_parse_from(["ask-bridge", "login", "--provider", "gemini"]).unwrap();
        assert_eq!(cli.provider, Some(Provider::Gemini));
        assert!(matches!(cli.command, Some(Commands::Login)));
    }

    #[test]
    fn parses_m365_provider_as_global_argument() {
        let before = Cli::try_parse_from(["ask-bridge", "--provider", "m365", "login"]).unwrap();
        assert_eq!(before.provider, Some(Provider::M365Copilot));
        assert!(matches!(before.command, Some(Commands::Login)));

        let after = Cli::try_parse_from(["ask-bridge", "login", "--provider", "m365"]).unwrap();
        assert_eq!(after.provider, Some(Provider::M365Copilot));
        assert!(matches!(after.command, Some(Commands::Login)));
    }

    #[test]
    fn parses_config_command() {
        let cli = Cli::try_parse_from(["ask-bridge", "config", "--provider", "gemini"]).unwrap();
        assert_eq!(cli.provider, Some(Provider::Gemini));
        assert!(matches!(cli.command, Some(Commands::Config)));
    }

    #[test]
    fn parses_config_command_without_provider() {
        let cli = Cli::try_parse_from(["ask-bridge", "config"]).unwrap();
        assert_eq!(cli.provider, None);
        assert!(matches!(cli.command, Some(Commands::Config)));
    }

    #[test]
    fn parses_update_command() {
        let cli = Cli::try_parse_from(["ask-bridge", "update"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Update)));
    }

    #[test]
    fn leaves_provider_unset_when_cli_argument_is_missing() {
        let cli = Cli::try_parse_from(["ask-bridge", "hello"]).unwrap();
        assert_eq!(cli.provider, None);
    }

    #[test]
    fn parses_provider_from_config_json() {
        assert_eq!(
            parse_configured_provider(r#"{"provider":"gemini"}"#).unwrap(),
            Some(Provider::Gemini)
        );
        assert_eq!(
            parse_configured_provider(r#"{"provider":"chatgpt"}"#).unwrap(),
            Some(Provider::ChatGpt)
        );
        assert_eq!(
            parse_configured_provider(r#"{"provider":"chat-gpt"}"#).unwrap(),
            Some(Provider::ChatGpt)
        );
        assert_eq!(
            parse_configured_provider(r#"{"provider":"claude"}"#).unwrap(),
            Some(Provider::Claude)
        );
        assert_eq!(
            parse_configured_provider(r#"{"provider":"claude-ai"}"#).unwrap(),
            Some(Provider::Claude)
        );
        for alias in [
            "m365",
            "m365-copilot",
            "m365_copilot",
            "microsoft365",
            "microsoft-365-copilot",
        ] {
            let config = format!(r#"{{"provider":"{alias}"}}"#);
            assert_eq!(
                parse_configured_provider(&config).unwrap(),
                Some(Provider::M365Copilot)
            );
        }
        assert_eq!(Provider::M365Copilot.to_string(), "m365");
        assert_eq!(parse_configured_provider(r#"{}"#).unwrap(), None);
    }

    #[test]
    fn resolves_provider_precedence() {
        assert_eq!(
            effective_provider(Some(Provider::ChatGpt), Some(Provider::Gemini)),
            Provider::ChatGpt
        );
        assert_eq!(
            effective_provider(None, Some(Provider::Gemini)),
            Provider::Gemini
        );
        assert_eq!(effective_provider(None, None), Provider::ChatGpt);
    }

    #[test]
    fn cli_provider_bypasses_invalid_config() {
        let provider = resolve_provider_with(Some(Provider::ChatGpt), || {
            Err("config should not be loaded".to_string())
        })
        .unwrap();

        assert_eq!(provider, Provider::ChatGpt);
    }

    #[test]
    fn resolves_provider_from_config_when_cli_provider_is_missing() {
        let provider = resolve_provider_with(None, || Ok(Some(Provider::Gemini))).unwrap();
        assert_eq!(provider, Provider::Gemini);
    }

    #[test]
    fn rejects_invalid_provider_in_config_json() {
        let err = parse_configured_provider(r#"{"provider":"copilot"}"#).unwrap_err();
        assert!(err.contains("Invalid provider"));
    }

    #[test]
    fn parses_close_command() {
        let cli = Cli::try_parse_from(["ask-bridge", "close"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Close)));
    }

    #[test]
    fn hides_debug_commands_from_help() {
        let mut command = Cli::command();
        let help = command.render_long_help().to_string();

        assert!(!help.contains("\n  open"));
        assert!(!help.contains("\n  get"));
        assert!(!help.contains("\n  dump"));
        assert!(!help.contains("\n  screenshot"));
        assert!(help.contains("\n  login"));
        assert!(help.contains("\n  close"));
        assert!(help.contains("\n  update"));
        assert!(help.contains("m365"));
    }

    #[test]
    fn still_parses_hidden_debug_commands() {
        let cli = Cli::try_parse_from(["ask-bridge", "open"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Open { .. })));

        let cli = Cli::try_parse_from(["ask-bridge", "get"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Get { .. })));

        let cli = Cli::try_parse_from(["ask-bridge", "dump"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Dump)));

        let cli = Cli::try_parse_from(["ask-bridge", "screenshot"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Screenshot)));
    }

    #[test]
    fn parses_verbose_get_command_flag() {
        let url = "https://chatgpt.com/c/6a50fe34-43c0-83ee-ab86-d41adf91625e";
        let cli = Cli::try_parse_from(["ask-bridge", "get", "--verbose", url]).unwrap();
        if let Some(Commands::Get {
            url: parsed_url,
            verbose,
        }) = cli.command
        {
            assert_eq!(parsed_url, Some(url.to_string()));
            assert!(verbose);
        } else {
            panic!("expected get command");
        }
        assert!(!cli.verbose);
    }

    #[test]
    fn rejects_unknown_provider() {
        assert!(Cli::try_parse_from(["ask-bridge", "--provider", "copilot", "hello"]).is_err());
    }

    #[test]
    fn parses_claude_provider_argument() {
        let cli = Cli::try_parse_from(["ask-bridge", "--provider", "claude", "hello"]).unwrap();
        assert_eq!(cli.provider, Some(Provider::Claude));
    }

    #[test]
    fn parses_session_id_alias_and_rejects_new_session_conflict() {
        let cli = Cli::try_parse_from([
            "ask-bridge",
            "--provider",
            "chatgpt",
            "--session-id",
            "conversation-123",
            "continue",
        ])
        .unwrap();
        assert_eq!(cli.session_id.as_deref(), Some("conversation-123"));

        assert!(
            Cli::try_parse_from([
                "ask-bridge",
                "--new",
                "--session",
                "conversation-123",
                "continue",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "ask-bridge",
                "--session",
                "conversation-123",
                "--session-url",
                "https://chatgpt.com/c/conversation-123",
                "continue",
            ])
            .is_err()
        );
    }

    #[test]
    fn maps_provider_urls() {
        assert_eq!(
            Provider::from_url("https://chatgpt.com/c/abc"),
            Some(Provider::ChatGpt)
        );
        assert_eq!(
            Provider::from_url("https://gemini.google.com/app/abc"),
            Some(Provider::Gemini)
        );
        assert_eq!(
            Provider::from_url("https://claude.ai/chat/abc"),
            Some(Provider::Claude)
        );
        for url in [
            "https://m365.cloud.microsoft/chat",
            "https://www.m365.cloud.microsoft/chat",
            "https://m365copilot.com",
            "https://www.m365copilot.com/chat",
        ] {
            assert_eq!(Provider::from_url(url), Some(Provider::M365Copilot));
        }
        for url in [
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            "https://office.com/",
            "https://www.office.com/",
            "https://tenant.example.com/login",
            "https://m365.cloud.microsoft.example.com/chat",
            "http://m365.cloud.microsoft/chat",
        ] {
            assert_eq!(Provider::from_url(url), None);
        }
        assert_eq!(Provider::from_url("https://example.com"), None);
        assert_eq!(
            Provider::from_url("https://example.com/?next=https://chatgpt.com/c/abc"),
            None
        );
        assert_eq!(Provider::from_url("http://chatgpt.com/c/abc"), None);
    }

    #[test]
    fn resolves_session_ids_for_each_provider() {
        assert_eq!(
            resolve_session_target(
                Provider::ChatGpt,
                true,
                session(SessionInputKind::Id, "chat-123"),
            )
            .unwrap(),
            (
                Provider::ChatGpt,
                "https://chatgpt.com/c/chat-123".to_string()
            )
        );
        assert_eq!(
            resolve_session_target(
                Provider::Gemini,
                true,
                session(SessionInputKind::Id, "gemini_123"),
            )
            .unwrap(),
            (
                Provider::Gemini,
                "https://gemini.google.com/app/gemini_123".to_string()
            )
        );
        assert_eq!(
            resolve_session_target(
                Provider::Claude,
                true,
                session(SessionInputKind::Id, "claude-123"),
            )
            .unwrap(),
            (
                Provider::Claude,
                "https://claude.ai/chat/claude-123".to_string()
            )
        );
    }

    #[test]
    fn session_url_infers_provider_unless_cli_provider_conflicts() {
        let target = resolve_session_target(
            Provider::Gemini,
            false,
            session(SessionInputKind::Url, "https://chatgpt.com/c/chat-123"),
        )
        .unwrap();
        assert_eq!(target.0, Provider::ChatGpt);
        assert_eq!(target.1, "https://chatgpt.com/c/chat-123");

        let error = resolve_session_target(
            Provider::Gemini,
            true,
            session(SessionInputKind::Url, "https://chatgpt.com/c/chat-123"),
        )
        .unwrap_err();
        assert!(error.contains("but --provider selected Gemini"));
    }

    #[test]
    fn rejects_invalid_session_urls_and_ids() {
        for invalid in [
            "https://example.com/c/chat-123",
            "https://chatgpt.com/",
            "https://gemini.google.com/app",
            "https://claude.ai/chat/",
            "../chat-123",
            "chat/123",
        ] {
            assert!(
                resolve_session_target(
                    Provider::ChatGpt,
                    false,
                    session(SessionInputKind::Auto, invalid),
                )
                .is_err(),
                "expected {invalid:?} to be rejected"
            );
        }
    }

    #[test]
    fn supports_m365_session_urls_but_rejects_raw_ids() {
        let raw_error = resolve_session_target(
            Provider::M365Copilot,
            true,
            session(SessionInputKind::Id, "conversation-123"),
        )
        .unwrap_err();
        assert!(raw_error.contains("raw session IDs"));
        assert!(raw_error.contains("--session-id"));

        let url = Url::parse("https://m365.cloud.microsoft/chat/conversation/abc").unwrap();
        assert!(Provider::M365Copilot.owns_conversation_url(&url));
        assert_eq!(
            resolve_session_target(
                Provider::M365Copilot,
                true,
                session(SessionInputKind::Url, url.as_str()),
            )
            .unwrap(),
            (Provider::M365Copilot, url.to_string())
        );
    }

    #[test]
    fn m365_capabilities_are_windows_only_experimental() {
        assert_eq!(
            Provider::M365Copilot.capabilities_for_platform(true),
            ProviderCapabilities {
                session: SessionSupport::UrlOnly,
                images: true,
                files: true,
                model_selection: true,
                reasoning: true,
                image_download: true,
            }
        );
        assert_eq!(
            Provider::M365Copilot.capabilities_for_platform(false),
            ProviderCapabilities {
                session: SessionSupport::None,
                images: false,
                files: false,
                model_selection: false,
                reasoning: false,
                image_download: false,
            }
        );
    }

    #[test]
    fn non_windows_rejects_m365_v2_features_with_platform_error() {
        for (flag, value) in [
            (
                "--session-url",
                "https://m365.cloud.microsoft/chat/conversation/abc",
            ),
            ("--image", "token.png"),
            ("--file", "token.txt"),
            ("--model", "GPT 5.5"),
            ("--reasoning", "quick"),
            ("--image-output", "images"),
        ] {
            let cli =
                Cli::try_parse_from(["ask-bridge", "--provider", "m365", flag, value, "read"])
                    .unwrap();
            let error =
                validate_provider_feature_support_for_platform(Provider::M365Copilot, &cli, false)
                    .unwrap_err();
            assert!(error.contains(flag));
            assert!(error.contains("Windows-only experimental"));
        }
    }

    #[test]
    fn validates_m365_conversation_url_ownership_fail_closed() {
        for valid in [
            "https://m365.cloud.microsoft/chat/conversation/abc",
            "https://www.m365.cloud.microsoft/chat/conversation/abc-123_DEF",
        ] {
            assert!(Provider::M365Copilot.owns_conversation_url(&Url::parse(valid).unwrap()));
        }
        for invalid in [
            "http://m365.cloud.microsoft/chat/conversation/abc",
            "https://m365copilot.com/chat/conversation/abc",
            "https://m365.cloud.microsoft/chat",
            "https://m365.cloud.microsoft/chat/conversation/",
            "https://m365.cloud.microsoft/chat/conversation/abc/extra",
            "https://m365.cloud.microsoft/chat/conversation/abc?tenant=secret",
            "https://m365.cloud.microsoft/chat/conversation/abc#fragment",
            "https://m365.cloud.microsoft/chat/conversation/%2Fadmin",
            "https://m365.cloud.microsoft.evil.test/chat/conversation/abc",
        ] {
            assert!(
                !Provider::M365Copilot.owns_conversation_url(&Url::parse(invalid).unwrap()),
                "expected {invalid} to be rejected"
            );
        }
    }

    #[test]
    fn infers_m365_session_url_provider_and_reports_explicit_conflicts() {
        let url =
            Url::parse("https://m365.cloud.microsoft/chat/conversation/conversation-123").unwrap();
        assert_eq!(
            resolve_session_url_provider(Provider::ChatGpt, false, &url).unwrap(),
            Provider::M365Copilot
        );

        let error = resolve_session_url_provider(Provider::ChatGpt, true, &url).unwrap_err();
        assert!(error.contains("Microsoft 365 Copilot"));
        assert!(error.contains("--provider selected ChatGPT"));
    }

    #[test]
    fn session_support_levels_distinguish_urls_and_raw_ids() {
        assert!(!SessionSupport::None.supports_url());
        assert!(!SessionSupport::None.supports_id());
        assert!(SessionSupport::UrlOnly.supports_url());
        assert!(!SessionSupport::UrlOnly.supports_id());
        assert!(SessionSupport::UrlAndId.supports_url());
        assert!(SessionSupport::UrlAndId.supports_id());
    }

    #[test]
    fn parses_chatgpt_agent_prompt_with_chinese_agent_name() {
        assert_eq!(
            parse_chatgpt_agent_prompt(
                "@智慧 研究多奇數位創意有限公司的發展沿革與創辦人的豐功偉業"
            ),
            Some(ChatGptAgentPrompt {
                agent_mention: "@智慧",
                body: "研究多奇數位創意有限公司的發展沿革與創辦人的豐功偉業"
            })
        );
    }

    #[test]
    fn parses_chatgpt_agent_prompt_with_ten_character_agent_name() {
        assert_eq!(
            parse_chatgpt_agent_prompt("@一二三四五六七八九十 查資料"),
            Some(ChatGptAgentPrompt {
                agent_mention: "@一二三四五六七八九十",
                body: "查資料"
            })
        );
    }

    #[test]
    fn trims_extra_whitespace_between_chatgpt_agent_and_body() {
        assert_eq!(
            parse_chatgpt_agent_prompt("@智慧 \n\t查資料").unwrap().body,
            "查資料"
        );
    }

    #[test]
    fn rejects_invalid_chatgpt_agent_prompt_shapes() {
        assert_eq!(parse_chatgpt_agent_prompt("智慧 查資料"), None);
        assert_eq!(parse_chatgpt_agent_prompt("@ 查資料"), None);
        assert_eq!(parse_chatgpt_agent_prompt("@智慧"), None);
        assert_eq!(parse_chatgpt_agent_prompt("@智慧   "), None);
        assert_eq!(
            parse_chatgpt_agent_prompt("@一二三四五六七八九十甲 查資料"),
            None
        );
    }

    #[test]
    fn extracts_snapshot_uid_from_common_formats() {
        assert_eq!(
            extract_snapshot_uid(r#"- button "上傳檔案" [uid="1_23"]"#),
            Some("1_23".to_string())
        );
        assert_eq!(
            extract_snapshot_uid(r#"- button "Upload file" uid=42"#),
            Some("42".to_string())
        );
    }

    #[test]
    fn finds_snapshot_uid_with_include_and_exclude_terms() {
        let snapshot = r#"
            - button "加入雲端硬碟檔案" [uid="1_10"]
            - menuitem "上傳檔案. 文件、資料、程式碼檔案" [uid="1_11"]
        "#;
        assert_eq!(
            find_snapshot_uid(snapshot, &["上傳檔案"], &["雲端"]),
            Some("1_11".to_string())
        );
    }

    #[test]
    fn finds_m365_composer_uid_without_matching_search() {
        let snapshot = r#"
            - textbox "Search Microsoft 365 Copilot" [uid="1_20"]
            - textbox "傳送訊息給 Copilot" [uid="1_21"]
        "#;
        assert_eq!(find_m365_composer_uid(snapshot), Some("1_21".to_string()));
    }

    #[test]
    fn rejects_gemini_image_attachments() {
        let cli = Cli::try_parse_from([
            "ask-bridge",
            "--provider",
            "gemini",
            "--image",
            "token.png",
            "read",
        ])
        .unwrap();
        assert!(validate_provider_feature_support(Provider::Gemini, &cli).is_err());
    }

    #[test]
    fn allows_claude_image_and_file_attachments() {
        let cli = Cli::try_parse_from([
            "ask-bridge",
            "--provider",
            "claude",
            "--image",
            "token.png",
            "--file",
            "token.txt",
            "read",
        ])
        .unwrap();
        assert!(validate_provider_feature_support(Provider::Claude, &cli).is_ok());
    }

    #[test]
    fn allows_gemini_file_attachments() {
        let cli = Cli::try_parse_from([
            "ask-bridge",
            "--provider",
            "gemini",
            "--file",
            "token.txt",
            "read",
        ])
        .unwrap();
        assert!(validate_provider_feature_support(Provider::Gemini, &cli).is_ok());
    }

    #[test]
    fn preserves_existing_provider_capabilities() {
        assert_eq!(
            Provider::ChatGpt.capabilities(),
            ProviderCapabilities {
                session: SessionSupport::UrlAndId,
                images: true,
                files: true,
                model_selection: true,
                reasoning: true,
                image_download: true,
            }
        );
        assert_eq!(
            Provider::Gemini.capabilities(),
            ProviderCapabilities {
                session: SessionSupport::UrlAndId,
                images: false,
                files: true,
                model_selection: true,
                reasoning: true,
                image_download: true,
            }
        );
        assert_eq!(
            Provider::Claude.capabilities(),
            ProviderCapabilities {
                session: SessionSupport::UrlAndId,
                images: true,
                files: true,
                model_selection: true,
                reasoning: false,
                image_download: true,
            }
        );
        assert_eq!(
            Provider::M365Copilot.capabilities(),
            Provider::M365Copilot.capabilities_for_platform(cfg!(target_os = "windows"))
        );
    }

    #[test]
    fn windows_allows_m365_v2_features_but_rejects_raw_session_ids() {
        for (flag, value) in [
            (
                "--session-url",
                "https://m365.cloud.microsoft/chat/conversation/abc",
            ),
            ("--image", "token.png"),
            ("--file", "token.txt"),
            ("--model", "GPT 5.5"),
            ("--reasoning", "quick"),
            ("--image-output", "images"),
        ] {
            let cli =
                Cli::try_parse_from(["ask-bridge", "--provider", "m365", flag, value, "read"])
                    .unwrap();
            validate_provider_feature_support_for_platform(Provider::M365Copilot, &cli, true)
                .unwrap();
        }

        let cli = Cli::try_parse_from([
            "ask-bridge",
            "--provider",
            "m365",
            "--session-id",
            "conversation-123",
            "read",
        ])
        .unwrap();
        let error =
            validate_provider_feature_support_for_platform(Provider::M365Copilot, &cli, true)
                .unwrap_err();
        assert!(error.contains("--session-id"));
    }

    #[test]
    fn parses_reasoning_cli_argument() {
        let cli = Cli::try_parse_from([
            "ask-bridge",
            "--provider",
            "chatgpt",
            "--model",
            "GPT-5.6 Sol",
            "--reasoning",
            "high",
            "solve",
        ])
        .unwrap();

        assert_eq!(cli.model.as_deref(), Some("GPT-5.6 Sol"));
        assert_eq!(cli.reasoning.as_deref(), Some("high"));
    }

    #[test]
    fn resolves_chatgpt_model_and_reasoning_independently() {
        let plan =
            resolve_selection_plan(Provider::ChatGpt, Some("GPT-5.6 Sol"), Some("medium")).unwrap();

        assert_eq!(plan.model.as_deref(), Some("GPT-5.6 Sol"));
        assert_eq!(plan.reasoning, Some(ReasoningRequest::ChatGptMedium));
        assert!(!plan.used_legacy_model);
    }

    #[test]
    fn resolves_chatgpt_reasoning_aliases() {
        for (value, expected) in [
            ("auto", ReasoningRequest::ChatGptAuto),
            ("自動", ReasoningRequest::ChatGptAuto),
            ("智慧", ReasoningRequest::ChatGptAuto),
            ("instant", ReasoningRequest::ChatGptInstant),
            ("即時", ReasoningRequest::ChatGptInstant),
            ("medium", ReasoningRequest::ChatGptMedium),
            ("中", ReasoningRequest::ChatGptMedium),
            ("中等", ReasoningRequest::ChatGptMedium),
            ("high", ReasoningRequest::ChatGptHigh),
            ("高", ReasoningRequest::ChatGptHigh),
        ] {
            let plan = resolve_selection_plan(Provider::ChatGpt, None, Some(value)).unwrap();
            assert_eq!(plan.reasoning, Some(expected), "unexpected alias {value}");
        }
    }

    #[test]
    fn converts_legacy_reasoning_like_model_values() {
        let chatgpt = resolve_selection_plan(Provider::ChatGpt, Some("高"), None).unwrap();
        assert_eq!(chatgpt.model, None);
        assert_eq!(chatgpt.reasoning, Some(ReasoningRequest::ChatGptHigh));
        assert!(chatgpt.used_legacy_model);

        let gemini = resolve_selection_plan(Provider::Gemini, Some("延伸思考"), None).unwrap();
        assert_eq!(gemini.model, None);
        assert_eq!(gemini.reasoning, Some(ReasoningRequest::GeminiExtended));
        assert!(gemini.used_legacy_model);
    }

    #[test]
    fn validates_gemini_extended_thinking_combinations() {
        let plan =
            resolve_selection_plan(Provider::Gemini, Some("3.1 Pro"), Some("extended")).unwrap();
        assert_eq!(plan.model.as_deref(), Some("3.1 Pro"));
        assert_eq!(plan.reasoning, Some(ReasoningRequest::GeminiExtended));

        let error = resolve_selection_plan(Provider::Gemini, Some("3.6 Flash"), Some("extended"))
            .unwrap_err();
        assert!(error.contains("incompatible"));
    }

    #[test]
    fn rejects_unsupported_or_ambiguous_reasoning_values() {
        let unsupported =
            resolve_selection_plan(Provider::ChatGpt, None, Some("ultra")).unwrap_err();
        assert!(unsupported.contains("auto, instant, medium, high"));

        let ambiguous =
            resolve_selection_plan(Provider::ChatGpt, Some("高"), Some("high")).unwrap_err();
        assert!(ambiguous.contains("cannot be combined"));
    }

    #[test]
    fn rejects_claude_reasoning_without_changing_model_selection() {
        let error =
            resolve_selection_plan(Provider::Claude, Some("Sonnet"), Some("high")).unwrap_err();
        assert!(error.contains("Claude"));
        assert!(error.contains("--reasoning"));

        let plan = resolve_selection_plan(Provider::Claude, Some("Sonnet"), None).unwrap();
        assert_eq!(plan.model.as_deref(), Some("Sonnet"));
        assert_eq!(plan.reasoning, None);
    }

    #[test]
    fn resolves_observed_m365_models_and_reasoning_aliases() {
        for (value, expected) in [
            ("GPT 5.6", "GPT 5.6"),
            ("gpt-5.5", "GPT 5.5"),
            ("Claude Sonnet", "Sonnet"),
            ("opus", "Opus"),
        ] {
            let plan = resolve_selection_plan(Provider::M365Copilot, Some(value), None).unwrap();
            assert_eq!(plan.model.as_deref(), Some(expected));
            assert_eq!(plan.reasoning, None);
        }

        for (value, expected) in [
            ("auto", ReasoningRequest::M365Auto),
            ("自動", ReasoningRequest::M365Auto),
            ("quick", ReasoningRequest::M365Quick),
            ("quick-response", ReasoningRequest::M365Quick),
            ("快速回應", ReasoningRequest::M365Quick),
            ("deep", ReasoningRequest::M365ThinkDeeper),
            ("think-deeper", ReasoningRequest::M365ThinkDeeper),
            ("深度思考", ReasoningRequest::M365ThinkDeeper),
        ] {
            let plan = resolve_selection_plan(Provider::M365Copilot, None, Some(value)).unwrap();
            assert_eq!(plan.model, None);
            assert_eq!(plan.reasoning, Some(expected));
        }
    }

    #[test]
    fn rejects_unknown_or_conflicting_m365_selection_values() {
        let empty = resolve_selection_plan(Provider::M365Copilot, Some("  "), None).unwrap_err();
        assert!(empty.contains("Empty model"));

        let model =
            resolve_selection_plan(Provider::M365Copilot, Some("future-model"), None).unwrap_err();
        assert!(model.contains("GPT 5.6, GPT 5.5, Sonnet, Opus"));

        let reasoning =
            resolve_selection_plan(Provider::M365Copilot, None, Some("maximum")).unwrap_err();
        assert!(reasoning.contains("auto, quick, think-deeper"));

        let conflict =
            resolve_selection_plan(Provider::M365Copilot, Some("GPT 5.6"), Some("think-deeper"))
                .unwrap_err();
        assert!(conflict.contains("share one UI control"));
    }

    #[test]
    fn classifies_m365_selection_locked_timeout_and_authentication_errors() {
        let locked = interpret_selection_status(
            SelectionKind::Model,
            "error: Opus is locked or unavailable",
        )
        .unwrap_err();
        assert!(locked.contains("model selection"));
        assert!(locked.contains("locked or unavailable"));

        let timeout = interpret_selection_status(SelectionKind::Model, "pending").unwrap_err();
        assert!(timeout.contains("timed out"));

        let authentication = interpret_selection_status(
            SelectionKind::Reasoning,
            "error: authentication: Microsoft sign-in expired during selection",
        )
        .unwrap_err();
        assert!(authentication.contains("reasoning selection"));
        assert!(authentication.contains("authentication"));
    }

    #[test]
    fn converts_legacy_m365_reasoning_like_model_values() {
        let plan =
            resolve_selection_plan(Provider::M365Copilot, Some("Quick response"), None).unwrap();
        assert_eq!(plan.model, None);
        assert_eq!(plan.reasoning, Some(ReasoningRequest::M365Quick));
        assert!(plan.used_legacy_model);
    }

    #[test]
    fn preserves_claude_model_selector_script() {
        let target = serde_json::to_string("Sonnet").unwrap();
        let script = claude_model_switch_script(&target);

        assert!(script.contains(r#"[data-testid="model-selector-dropdown"]"#));
        assert!(script.contains("model|claude|opus|sonnet|haiku|fable"));
        assert!(script.contains("startsWith(target)"));
        assert!(script.contains(r#"const target = norm("Sonnet");"#));
    }

    #[test]
    fn preserves_provider_baselines_without_reasoning() {
        for provider in [
            Provider::ChatGpt,
            Provider::Gemini,
            Provider::Claude,
            Provider::M365Copilot,
        ] {
            let plan = resolve_selection_plan(provider, None, None).unwrap();
            assert_eq!(plan, SelectionPlan::default());
        }
    }

    #[test]
    fn validates_m365_attachment_guarantees_and_passthrough() {
        let root = make_test_dir("m365_attachments");
        std::fs::create_dir_all(&root).unwrap();
        let png = root.join("sample.png");
        let jpeg = root.join("sample.jpeg");
        let pdf = root.join("sample.pdf");
        let docx = root.join("sample.docx");
        let text = root.join("sample.txt");
        let csv = root.join("sample.csv");
        let custom = root.join("sample.custom");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\npayload").unwrap();
        std::fs::write(&jpeg, [0xff, 0xd8, 0xff, 0xe0]).unwrap();
        std::fs::write(&pdf, b"%PDF-1.7\n").unwrap();
        std::fs::write(&docx, b"PK\x03\x04").unwrap();
        std::fs::write(&text, b"safe fixture").unwrap();
        std::fs::write(&csv, b"name,value\nsample,1").unwrap();
        std::fs::write(&custom, b"provider-defined format").unwrap();

        for path in [&png, &jpeg] {
            validate_attachment_path(
                Provider::M365Copilot,
                AttachmentKind::Image,
                path.to_str().unwrap(),
            )
            .unwrap();
        }
        for path in [&pdf, &docx, &text, &csv, &custom] {
            validate_attachment_path(
                Provider::M365Copilot,
                AttachmentKind::File,
                path.to_str().unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn rejects_invalid_m365_attachment_paths_and_content() {
        let root = make_test_dir("m365_invalid_attachments");
        std::fs::create_dir_all(&root).unwrap();
        let unsupported_image = root.join("sample.gif");
        let empty_image = root.join("empty.png");
        let corrupt_image = root.join("corrupt.jpg");
        let corrupt_pdf = root.join("corrupt.pdf");
        let corrupt_docx = root.join("corrupt.docx");
        let missing = root.join("missing.pdf");
        std::fs::write(&unsupported_image, b"GIF89a").unwrap();
        std::fs::write(&empty_image, b"").unwrap();
        std::fs::write(&corrupt_image, b"not-a-jpeg").unwrap();
        std::fs::write(&corrupt_pdf, b"not-a-pdf").unwrap();
        std::fs::write(&corrupt_docx, b"not-a-docx").unwrap();

        for (kind, path) in [
            (AttachmentKind::Image, unsupported_image.as_path()),
            (AttachmentKind::Image, empty_image.as_path()),
            (AttachmentKind::Image, corrupt_image.as_path()),
            (AttachmentKind::File, corrupt_pdf.as_path()),
            (AttachmentKind::File, corrupt_docx.as_path()),
            (AttachmentKind::File, root.as_path()),
            (AttachmentKind::File, missing.as_path()),
        ] {
            assert!(
                validate_attachment_path(Provider::M365Copilot, kind, path.to_str().unwrap())
                    .is_err()
            );
        }
        assert!(validate_attachment_path(Provider::M365Copilot, AttachmentKind::File, "").is_err());
    }

    #[test]
    fn matches_accept_rules_by_mime_wildcard_and_extension() {
        assert!(accept_rule_matches(
            "application/pdf",
            "brief.pdf",
            "application/pdf"
        ));
        assert!(accept_rule_matches("image/*", "image.png", "image/png"));
        assert!(accept_rule_matches(
            ".docx,.txt",
            "REPORT.DOCX",
            "application/octet-stream"
        ));
        assert!(accept_rule_matches(".csv", "data.csv", "text/csv"));
        assert!(accept_rule_matches(
            "",
            "sample.custom",
            "application/octet-stream"
        ));
        assert!(!accept_rule_matches(".pdf", "brief.txt", "text/plain"));
    }

    #[test]
    fn classifies_m365_attachment_and_policy_errors() {
        assert_eq!(
            classify_m365_error_message("authentication: Microsoft sign-in expired"),
            M365ErrorClass::Authentication
        );
        assert_eq!(
            classify_m365_error_message("Your organization DLP policy blocked this file"),
            M365ErrorClass::Policy
        );
        assert_eq!(
            classify_m365_error_message("upload control not found; this may be a UI rollout"),
            M365ErrorClass::Selector
        );
        assert_eq!(
            classify_m365_error_message("attachment upload timed out"),
            M365ErrorClass::Timeout
        );
        assert_eq!(
            classify_m365_error_message("unsupported attachment rejected"),
            M365ErrorClass::Rejected
        );
    }

    #[test]
    fn derives_image_extensions_from_bytes_and_avoids_overwrite() {
        assert_eq!(
            image_extension_from_bytes(b"\x89PNG\r\n\x1a\n", Some("image/jpeg")).unwrap(),
            "png"
        );
        assert_eq!(
            image_extension_from_bytes(&[0xff, 0xd8, 0xff], None).unwrap(),
            "jpg"
        );
        assert!(image_extension_from_bytes(b"not-image", Some("image/png")).is_err());

        let root = make_test_dir("m365_image_output");
        std::fs::create_dir_all(&root).unwrap();
        let requested = root.join("result.jpg");
        std::fs::write(root.join("result.png"), b"existing").unwrap();
        let output = explicit_image_output_path(requested.to_str().unwrap(), 1, 0, "png").unwrap();
        assert_eq!(
            output.file_name().and_then(|value| value.to_str()),
            Some("result_2.png")
        );

        let multi = explicit_image_output_path(requested.to_str().unwrap(), 2, 1, "webp").unwrap();
        assert_eq!(
            multi.file_name().and_then(|value| value.to_str()),
            Some("result_2.webp")
        );
    }

    #[test]
    fn m365_image_download_without_explicit_output_is_a_noop() {
        download_m365_images_from_latest_message("unused", None, false).unwrap();
    }

    #[test]
    fn interprets_m365_image_download_empty_partial_and_auth_results() {
        let no_images = serde_json::json!({
            "status": "success",
            "images": [],
            "failures": []
        });
        assert!(
            interpret_m365_image_download_result(&no_images)
                .unwrap_err()
                .contains("no generated image")
        );

        let all_failed = serde_json::json!({
            "status": "success",
            "images": [],
            "failures": [
                { "reason": "HTTP 403 from https://example.test/private.png" },
                { "reason": "unexpected content type text/html" }
            ]
        });
        let all_failed_error = interpret_m365_image_download_result(&all_failed).unwrap_err();
        assert!(all_failed_error.contains("all 2"));
        assert!(!all_failed_error.contains("example.test"));
        assert!(all_failed_error.contains("<url>"));

        let partial = serde_json::json!({
            "status": "success",
            "images": [{ "dataUrl": "data:image/png;base64,AA==" }],
            "failures": [{ "reason": "blob URL expired" }]
        });
        let (images, failures) = interpret_m365_image_download_result(&partial).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(failures, vec!["blob URL expired"]);

        let authentication = serde_json::json!({
            "status": "error",
            "error": "authentication: Microsoft sign-in expired during image download"
        });
        assert!(
            interpret_m365_image_download_result(&authentication)
                .unwrap_err()
                .starts_with("authentication:")
        );

        let policy = serde_json::json!({
            "status": "error",
            "error": "organization policy blocked image download"
        });
        assert!(
            interpret_m365_image_download_result(&policy)
                .unwrap_err()
                .starts_with("policy:")
        );
    }

    #[test]
    fn finds_linux_google_chrome_command_from_path() {
        let root = make_test_dir("chrome_path");
        let first_dir = root.join("first");
        let second_dir = root.join("second");
        std::fs::create_dir_all(&first_dir).unwrap();
        std::fs::create_dir_all(&second_dir).unwrap();

        let stable_path = first_dir.join("google-chrome-stable");
        let chrome_path = second_dir.join("google-chrome");
        std::fs::write(&stable_path, "").unwrap();
        std::fs::write(&chrome_path, "").unwrap();

        let path_env = std::env::join_paths([first_dir.as_os_str(), second_dir.as_os_str()])
            .expect("test PATH should be joinable");

        let found = find_linux_chrome_path(Some(path_env.as_os_str()), &[]);

        assert_eq!(found, Some(chrome_path.to_string_lossy().to_string()));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn finds_linux_chrome_from_standard_candidates_when_path_misses() {
        let root = make_test_dir("chrome_candidate");
        std::fs::create_dir_all(&root).unwrap();
        let chrome_path = root.join("google-chrome");
        std::fs::write(&chrome_path, "").unwrap();

        let chrome_path_str = chrome_path.to_string_lossy().to_string();
        let candidates = [chrome_path_str.as_str()];

        let found = find_linux_chrome_path(None, &candidates);

        assert_eq!(found, Some(chrome_path_str));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn returns_none_when_linux_chrome_is_missing() {
        assert_eq!(find_linux_chrome_path(None, &[]), None);
    }

    #[test]
    fn matches_profile_argument_with_quotes_and_slashes() {
        let command = r#""C:\Program Files\Google\Chrome\Application\chrome.exe" --remote-debugging-port=9223 "--user-data-dir=C:\Users\Will\.config\ask-bridge\chrome-profile""#;
        let profile_path = r"C:/Users/Will/.config/ask-bridge/chrome-profile";

        assert!(command_uses_profile(command, profile_path));
    }

    #[test]
    fn matches_profile_argument_when_value_is_separated_by_space() {
        let command = r#"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=9223 --user-data-dir /Users/will/.config/ask-bridge/chrome-profile"#;
        let profile_path = "/Users/will/.config/ask-bridge/chrome-profile";

        assert!(command_uses_profile(command, profile_path));
    }

    #[test]
    fn rejects_different_profile_argument() {
        let command = r#"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=9223 --user-data-dir=/Users/will/.config/other/chrome-profile"#;
        let profile_path = "/Users/will/.config/ask-bridge/chrome-profile";

        assert!(!command_uses_profile(command, profile_path));
    }

    #[test]
    fn rejects_profile_and_marker_prefixes_with_extra_suffixes() {
        let profile_path = r"C:\Users\Will\.config\ask-bridge\chrome-profile";
        let profile_copy =
            r#"chrome.exe --user-data-dir=C:\Users\Will\.config\ask-bridge\chrome-profile-copy"#;
        let marker_copy = "chrome.exe --ask-bridge-instance-copy";

        assert!(!command_uses_profile(profile_copy, profile_path));
        assert!(!command_identifies_ask_chrome(marker_copy, profile_path));
    }

    #[test]
    fn composer_without_account_or_auth_controls_has_logged_in_state() {
        let signals = LoginSignals {
            account: false,
            auth_control: false,
            auth_path: false,
            composer: true,
            stable: true,
        };

        assert_eq!(signals.state(Provider::ChatGpt), LoginState::LoggedIn);
    }

    #[test]
    fn chatgpt_login_signals_wait_for_ambiguous_auth_shell() {
        let script = Provider::ChatGpt.login_signals_js();

        assert!(script.starts_with("async () =>"));
        assert!(script.contains("earliestDecision"));
        assert!(script.contains("stableSince"));
        assert!(script.contains("let stable = false"));
        assert!(script.contains("JSON.stringify(nextSignals)"));
        assert!(script.contains("await new Promise"));
        assert!(script.contains("Date.now() + 5000"));
        assert!(script.contains("return { ...signals, stable }"));
    }

    #[test]
    fn account_control_has_logged_in_state() {
        let signals = LoginSignals {
            account: true,
            auth_control: false,
            auth_path: false,
            composer: true,
            stable: true,
        };

        assert_eq!(signals.state(Provider::ChatGpt), LoginState::LoggedIn);
    }

    #[test]
    fn auth_control_or_auth_path_has_logged_out_state() {
        let visible_auth_control = LoginSignals {
            account: false,
            auth_control: true,
            auth_path: false,
            composer: true,
            stable: true,
        };
        let auth_path = LoginSignals {
            account: false,
            auth_control: false,
            auth_path: true,
            composer: false,
            stable: false,
        };

        assert_eq!(
            visible_auth_control.state(Provider::ChatGpt),
            LoginState::LoggedOut
        );
        assert_eq!(auth_path.state(Provider::ChatGpt), LoginState::LoggedOut);
    }

    #[test]
    fn empty_login_signals_have_unknown_state() {
        let signals = LoginSignals {
            account: false,
            auth_control: false,
            auth_path: false,
            composer: false,
            stable: true,
        };

        assert_eq!(signals.state(Provider::ChatGpt), LoginState::Unknown);
    }

    #[test]
    fn unstable_chatgpt_signals_never_block_or_confirm_login() {
        for signals in [
            LoginSignals {
                account: false,
                auth_control: true,
                auth_path: false,
                composer: true,
                stable: false,
            },
            LoginSignals {
                account: false,
                auth_control: false,
                auth_path: false,
                composer: true,
                stable: false,
            },
        ] {
            assert_eq!(signals.state(Provider::ChatGpt), LoginState::Unknown);
        }
    }

    #[test]
    fn auth_path_overrides_stale_account_control() {
        let signals = LoginSignals {
            account: true,
            auth_control: false,
            auth_path: true,
            composer: true,
            stable: false,
        };

        assert_eq!(signals.state(Provider::ChatGpt), LoginState::LoggedOut);
    }

    #[test]
    fn gemini_composer_without_account_remains_unknown() {
        let signals = LoginSignals {
            account: false,
            auth_control: false,
            auth_path: false,
            composer: true,
            stable: true,
        };

        assert_eq!(signals.state(Provider::Gemini), LoginState::Unknown);
    }

    #[test]
    fn gemini_hidden_account_marker_is_logged_in() {
        let signals = LoginSignals {
            account: true, // script can detect marker even if hidden
            auth_control: false,
            auth_path: false,
            composer: true,
            stable: true,
        };

        assert_eq!(signals.state(Provider::Gemini), LoginState::LoggedIn);
    }

    #[test]
    fn claude_composer_without_account_remains_unknown() {
        let signals = LoginSignals {
            account: false,
            auth_control: false,
            auth_path: false,
            composer: true,
            stable: true,
        };

        assert_eq!(signals.state(Provider::Claude), LoginState::Unknown);
    }

    #[test]
    fn m365_composer_without_account_remains_unknown() {
        let signals = LoginSignals {
            account: false,
            auth_control: false,
            auth_path: false,
            composer: true,
            stable: true,
        };

        assert_eq!(signals.state(Provider::M365Copilot), LoginState::Unknown);
    }

    #[test]
    fn m365_account_and_auth_signals_are_fail_closed() {
        let logged_in = LoginSignals {
            account: true,
            auth_control: false,
            auth_path: false,
            composer: true,
            stable: true,
        };
        let logged_out = LoginSignals {
            account: false,
            auth_control: true,
            auth_path: true,
            composer: false,
            stable: true,
        };
        let unstable = LoginSignals {
            account: false,
            auth_control: true,
            auth_path: false,
            composer: true,
            stable: false,
        };

        assert_eq!(logged_in.state(Provider::M365Copilot), LoginState::LoggedIn);
        assert_eq!(
            logged_out.state(Provider::M365Copilot),
            LoginState::LoggedOut
        );
        assert_eq!(unstable.state(Provider::M365Copilot), LoginState::Unknown);
    }

    #[test]
    fn m365_login_scripts_include_observed_signals() {
        let ready = Provider::M365Copilot.ready_check_js();
        let login = Provider::M365Copilot.login_signals_js();

        for signal in [
            "login.microsoftonline.com",
            "login.live.com",
            "m365-chat-editor-target-element",
            "user-account-avatar",
            "loginfmt",
            "idSIButton9",
        ] {
            assert!(
                ready.contains(signal) || login.contains(signal),
                "missing M365 login signal {signal}"
            );
        }
        assert!(login.starts_with("async () =>"));
        assert!(login.contains("stableSince"));
        assert!(login.contains("return { ...signals, stable }"));
    }

    #[test]
    fn m365_selector_baseline_is_non_empty() {
        let provider = Provider::M365Copilot;
        for selector in [
            provider.assistant_selector(),
            provider.latest_response_selector(),
            provider.response_content_selector(),
            provider.composer_selectors_json(),
            provider.send_button_selectors_json(),
            provider.stop_button_selectors_json(),
        ] {
            assert!(!selector.trim().is_empty());
        }
        assert!(
            provider
                .assistant_selector()
                .contains("copilot-message-div")
        );
        assert!(
            provider
                .response_content_selector()
                .contains("markdown-reply")
        );
    }

    #[test]
    fn m365_completion_requires_stable_non_empty_text() {
        let mut tracker = ResponseCompletionTracker::default();

        assert!(!tracker.observe("generating", true, 10, 1, true));
        assert!(!tracker.observe("done", true, 0, 0, true));
        assert!(!tracker.observe("done", true, 10, 1, true));
        assert!(!tracker.observe("done", true, 12, 2, true));
        assert!(!tracker.observe("done", true, 12, 2, true));
        assert!(tracker.observe("done", true, 12, 2, true));
    }

    #[test]
    fn existing_provider_completion_keeps_three_done_polls() {
        let mut tracker = ResponseCompletionTracker::default();

        assert!(!tracker.observe("done", true, 0, 0, false));
        assert!(!tracker.observe("done", true, 0, 0, false));
        assert!(tracker.observe("done", true, 0, 0, false));
        assert!(!tracker.observe("waiting", true, 0, 0, false));
    }

    #[test]
    fn response_validation_rejects_empty_or_whitespace_content() {
        for content in ["", "   ", "\r\n\t"] {
            let error = validate_non_empty_response(Provider::M365Copilot, content.to_string())
                .unwrap_err();
            assert!(error.contains("empty response"));
        }

        assert_eq!(
            validate_non_empty_response(Provider::M365Copilot, "ok".to_string()).unwrap(),
            "ok"
        );
    }

    #[test]
    fn only_m365_explicit_image_output_can_continue_after_empty_markdown() {
        let error = "Microsoft 365 Copilot DOM scraper returned an empty response";
        assert!(allows_empty_markdown_for_explicit_image_output(
            Provider::M365Copilot,
            Some("images"),
            error
        ));
        assert!(!allows_empty_markdown_for_explicit_image_output(
            Provider::M365Copilot,
            None,
            error
        ));
        assert!(!allows_empty_markdown_for_explicit_image_output(
            Provider::ChatGpt,
            Some("images"),
            error
        ));
        assert!(!allows_empty_markdown_for_explicit_image_output(
            Provider::M365Copilot,
            Some("images"),
            "authentication failed"
        ));
    }

    #[test]
    fn prefers_logged_in_provider_page_over_selected_page() {
        let pages = [
            PageLoginState {
                id: 2,
                selected: true,
                login_state: LoginState::LoggedOut,
            },
            PageLoginState {
                id: 7,
                selected: false,
                login_state: LoginState::LoggedIn,
            },
        ];

        assert_eq!(preferred_provider_page_id(&pages), Some(7));
    }

    #[test]
    fn falls_back_to_selected_provider_page_when_none_are_logged_in() {
        let pages = [
            PageLoginState {
                id: 2,
                selected: false,
                login_state: LoginState::Unknown,
            },
            PageLoginState {
                id: 7,
                selected: true,
                login_state: LoginState::LoggedOut,
            },
        ];

        assert_eq!(preferred_provider_page_id(&pages), Some(7));
    }

    #[test]
    fn identifies_the_only_new_page_without_reusing_existing_provider_tabs() {
        let before = [
            Page {
                id: 1,
                url: "https://chatgpt.com/c/existing".to_string(),
                selected: true,
            },
            Page {
                id: 2,
                url: "https://example.com/".to_string(),
                selected: false,
            },
        ];
        let after = [
            Page {
                id: 1,
                url: "https://chatgpt.com/c/existing".to_string(),
                selected: false,
            },
            Page {
                id: 2,
                url: "https://example.com/".to_string(),
                selected: false,
            },
            Page {
                id: 7,
                url: "https://chatgpt.com/".to_string(),
                selected: true,
            },
        ];

        assert_eq!(unique_new_page_id(&before, &after), Ok(7));
    }

    #[test]
    fn refuses_to_guess_when_new_page_identity_is_ambiguous() {
        let before = [Page {
            id: 1,
            url: "https://chatgpt.com/c/existing".to_string(),
            selected: true,
        }];
        let after = [
            Page {
                id: 1,
                url: "https://chatgpt.com/c/existing".to_string(),
                selected: false,
            },
            Page {
                id: 7,
                url: "https://chatgpt.com/".to_string(),
                selected: true,
            },
            Page {
                id: 8,
                url: "https://example.com/popup".to_string(),
                selected: false,
            },
        ];

        let error = unique_new_page_id(&before, &after).unwrap_err();
        assert!(error.contains("Could not uniquely identify"));
        assert!(error.contains("[7, 8]"));
    }

    #[test]
    fn refuses_to_reuse_an_existing_page_when_no_new_page_appears() {
        let before = [Page {
            id: 1,
            url: "https://chatgpt.com/c/existing".to_string(),
            selected: true,
        }];
        let after = [Page {
            id: 1,
            url: "https://chatgpt.com/c/existing".to_string(),
            selected: true,
        }];

        let error = unique_new_page_id(&before, &after).unwrap_err();
        assert!(error.contains("Could not identify the newly opened tab"));
    }

    #[test]
    fn marker_identifies_ask_bridge_chrome_without_profile_argument() {
        let command = r#"chrome.exe --type=browser --ask-bridge-instance"#;

        assert!(command_identifies_ask_chrome(
            command,
            r"C:\Users\Will\.config\ask-bridge\chrome-profile"
        ));
    }

    #[test]
    fn parses_legacy_and_json_chrome_process_records() {
        assert_eq!(
            parse_chrome_process_record("15864\r\n"),
            Some(ChromeProcessRecord {
                pid: 15864,
                browser_id: None,
            })
        );
        assert_eq!(
            parse_chrome_process_record(r#"{"pid":20728,"browser_id":"browser-123"}"#),
            Some(ChromeProcessRecord {
                pid: 20728,
                browser_id: Some("browser-123".to_string()),
            })
        );
    }

    #[test]
    fn extracts_browser_id_from_cdp_version_response() {
        let body = r#"{"Browser":"Chrome/149","webSocketDebuggerUrl":"ws://127.0.0.1:9223/devtools/browser/browser-123"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length:{}\r\nContent-Type:application/json\r\n\r\n{}",
            body.len(),
            body
        );

        assert_eq!(
            browser_id_from_version_response(&response),
            Some("browser-123".to_string())
        );
        assert!(http_response_is_complete(response.as_bytes()));
        assert!(!http_response_is_complete(
            &response.as_bytes()[..response.len() - 1]
        ));

        let non_success = response.replacen("200 OK", "404 Not Found", 1);
        assert_eq!(browser_id_from_version_response(&non_success), None);
        assert_eq!(browser_id_from_version_response(body), None);

        let foreign_body = body.replace("127.0.0.1:9223", "example.com:9223");
        let foreign_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length:{}\r\n\r\n{}",
            foreign_body.len(),
            foreign_body
        );
        assert_eq!(browser_id_from_version_response(&foreign_response), None);

        let overflowing_length = format!(
            "HTTP/1.1 200 OK\r\nContent-Length:{}\r\n\r\n{{}}",
            usize::MAX
        );
        assert!(!http_response_is_complete(overflowing_length.as_bytes()));
    }

    #[test]
    fn build_chrome_process_record_prefers_unique_listener_pid() {
        let listeners = vec!["20728".to_string()];
        assert_eq!(
            build_chrome_process_record(&listeners, Some("browser-123")),
            Some(ChromeProcessRecord {
                pid: 20728,
                browser_id: Some("browser-123".to_string()),
            })
        );
    }

    #[test]
    fn build_chrome_process_record_requires_unambiguous_identity() {
        assert_eq!(
            build_chrome_process_record(
                &["20728".to_string(), "30000".to_string()],
                Some("browser-123")
            ),
            None
        );
        assert_eq!(
            build_chrome_process_record(&["20728".to_string()], None),
            None
        );
    }

    #[test]
    fn chrome_record_matches_current_checks_browser_identity_and_scope() {
        let record = ChromeProcessRecord {
            pid: 20728,
            browser_id: Some("browser-123".to_string()),
        };
        let single = vec!["20728".to_string()];
        let multiple = vec!["20728".to_string(), "30000".to_string()];

        assert!(chrome_record_matches_current(
            Some(&record),
            Some("browser-123"),
            &single
        ));
        assert!(!chrome_record_matches_current(
            Some(&record),
            Some("browser-456"),
            &single
        ));
        assert!(!chrome_record_matches_current(
            Some(&record),
            Some("browser-123"),
            &multiple
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_netstat_parser_matches_exact_listening_port() {
        let output = concat!(
            "  TCP    127.0.0.1:9223    0.0.0.0:0    LISTENING    20728\r\n",
            "  TCP    127.0.0.1:92230   0.0.0.0:0    LISTENING    30000\r\n",
            "  TCP    [::1]:9223        [::]:0       LISTENING    20728\r\n",
            "  TCP    127.0.0.1:9223    127.0.0.1:50000 ESTABLISHED 40000\r\n",
            "  UDP    127.0.0.1:9223    *:*                       50000\r\n"
        );

        assert_eq!(
            parse_windows_netstat_listener_pids(output, 9223),
            vec!["20728".to_string()]
        );
    }

    #[test]
    fn finds_ask_owner_pids_and_deduplicates_results() {
        let listeners = vec![
            "30000".to_string(),
            "20728".to_string(),
            "20728".to_string(),
        ];
        let commands = std::collections::HashMap::from([
            ("20728", "chrome.exe --type=utility"),
            ("30000", "chrome.exe --type=gpu-process"),
            (
                "18000",
                "chrome.exe --remote-debugging-port=9223 --ask-bridge-instance",
            ),
            (
                "15000",
                "chrome.exe --user-data-dir=C:\\Users\\Chris\\.config\\ask-bridge\\chrome-profile",
            ),
        ]);
        let parents = std::collections::HashMap::from([
            ("20728", "18000"),
            ("30000", "18000"),
            ("18000", "1"),
            ("15000", "1"),
        ]);

        let ask_pids = find_ask_chrome_owner_pids_with(
            &listeners,
            r"C:\Users\Chris\.config\ask-bridge\chrome-profile",
            |pid| commands.get(pid).map(|command| (*command).to_string()),
            |pid| parents.get(pid).map(|parent| (*parent).to_string()),
        );

        assert_eq!(ask_pids, vec!["18000".to_string()]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_wmic_value_after_blank_lines() {
        let output = "CommandLine\r\n\r\n  chrome.exe --remote-debugging-port=9223  \r\n\r\n";

        assert_eq!(
            parse_wmic_column_value(output),
            Some("chrome.exe --remote-debugging-port=9223".to_string())
        );
    }

    #[test]
    fn finds_ask_chrome_owner_in_parent_process_chain() {
        let commands = std::collections::HashMap::from([
            ("100", "chrome.exe --type=utility"),
            (
                "50",
                "chrome.exe --remote-debugging-port=9223 --ask-bridge-instance",
            ),
        ]);
        let parents = std::collections::HashMap::from([("100", "50"), ("50", "1")]);

        let owner = find_ask_chrome_owner_pid_with(
            "100",
            "/tmp/ask-bridge/chrome-profile",
            |pid| commands.get(pid).map(|command| (*command).to_string()),
            |pid| parents.get(pid).map(|parent| (*parent).to_string()),
        );

        assert_eq!(owner, Some("50".to_string()));
    }

    #[test]
    fn rejects_process_chain_without_profile_or_marker() {
        let commands = std::collections::HashMap::from([
            ("100", "chrome.exe --type=utility"),
            ("50", "chrome.exe --remote-debugging-port=9223"),
        ]);
        let parents = std::collections::HashMap::from([("100", "50"), ("50", "1")]);

        let owner = find_ask_chrome_owner_pid_with(
            "100",
            "/tmp/ask-bridge/chrome-profile",
            |pid| commands.get(pid).map(|command| (*command).to_string()),
            |pid| parents.get(pid).map(|parent| (*parent).to_string()),
        );

        assert_eq!(owner, None);
    }
}

fn read_clipboard() -> Result<String, String> {
    let output = Command::new("pbpaste")
        .output()
        .map_err(|e| format!("Failed to run pbpaste: {}", e))?;

    if !output.status.success() {
        return Err(format!("pbpaste exited with status: {}", output.status));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn write_clipboard(content: &str) -> Result<(), String> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run pbcopy: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(content.as_bytes())
            .map_err(|e| format!("Failed to write clipboard content: {}", e))?;
    }

    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait for pbcopy: {}", e))?;

    if !status.success() {
        return Err(format!("pbcopy exited with status: {}", status));
    }

    Ok(())
}

fn click_latest_copy_button(config_path: &str, provider: Provider) -> Result<(), String> {
    let response_selector = serde_json::to_string(provider.latest_response_selector())
        .map_err(|e| format!("Failed to serialize response selector: {}", e))?;
    let script = r#"() => {
                const isVisible = (el) => {
                    if (!el || el.disabled || el.getAttribute('aria-disabled') === 'true') return false;
                    const style = window.getComputedStyle(el);
                    if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
                    const rect = el.getBoundingClientRect();
                    return rect.width > 0 && rect.height > 0;
                };

                const labelOf = (el) => [
                    el.getAttribute('aria-label'),
                    el.getAttribute('title'),
                    el.getAttribute('data-testid'),
                    el.textContent
                ].filter(Boolean).join(' ');

                const isCopyButton = (el) => {
                    const label = labelOf(el);
                    return /copy|複製|复制|コピー|복사/i.test(label)
                        && !/prompt|提示詞|提示词|入力|table|表格/i.test(label);
                };
                const copyButtonScore = (el) => {
                    const label = labelOf(el);
                    if (!isCopyButton(el) || !isVisible(el)) return -1;
                    if (el.closest('pre, code, [class*="code"], [data-testid*="code"]')) return -1;
                    if (/copy-turn-action-button/i.test(label)) return 100;
                    if (/response|回應|回答|reply/i.test(label)) return 90;
                    if (el.closest('model-response, response-container, [data-message-author-role="assistant"], .agent-turn, [data-is-streaming], .font-claude-response')) return 50;
                    return 10;
                };
                const messages = Array.from(document.querySelectorAll(__RESPONSE_SELECTOR__));
                const latest = messages[messages.length - 1];
                if (!latest) return { ok: false, reason: "No assistant message found" };

                latest.scrollIntoView({ block: 'center', inline: 'nearest' });
                for (const type of ['pointerover', 'mouseover', 'mouseenter']) {
                    latest.dispatchEvent(new MouseEvent(type, { bubbles: true, view: window }));
                }

                const scopes = [
                    latest,
                    latest.closest('article'),
                    latest.closest('[data-testid^="conversation-turn"]'),
                    latest.parentElement,
                    latest.parentElement?.parentElement
                ].filter(Boolean);

                for (const scope of scopes) {
                    const buttons = Array.from(scope.querySelectorAll('button'));
                    const candidates = buttons
                        .map((button) => ({ button, score: copyButtonScore(button) }))
                        .filter((candidate) => candidate.score >= 0)
                        .sort((a, b) => b.score - a.score);
                    if (candidates.length > 0) {
                        const button = candidates[0].button;
                        button.click();
                        return { ok: true, label: labelOf(button) };
                    }
                }

                return { ok: false, reason: "Copy response button not found" };
            }"#
    .replace("__RESPONSE_SELECTOR__", &response_selector);
    let res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": script
        }),
    )?;

    let parsed = parse_script_result(&res)?;
    if parsed["ok"].as_bool().unwrap_or(false) {
        Ok(())
    } else {
        Err(parsed["reason"]
            .as_str()
            .unwrap_or("Failed to click copy response button")
            .to_string())
    }
}

fn wait_for_page_load(config_path: &str, provider: Provider, verbose: bool) -> Result<(), String> {
    if verbose {
        println!("Waiting for page readyState...");
    }

    // Phase 1: Wait for readyState complete or interactive
    let mut ready = false;
    for _ in 0..90 {
        let ready_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({
                "function": "() => document.readyState === 'complete' || document.readyState === 'interactive'"
            }),
        );

        if ready_res
            .and_then(|res| parse_script_result(&res))
            .map(|parsed| parsed.as_bool().unwrap_or(false))
            .unwrap_or(false)
        {
            ready = true;
            break;
        }

        thread::sleep(Duration::from_millis(500));
    }

    if !ready {
        return Err("Timeout waiting for page readyState to be loaded".to_string());
    }

    if verbose {
        println!("Waiting for {} page elements...", provider.display_name());
    }

    // Phase 2: Wait for key provider elements to render.
    for _ in 0..60 {
        let element_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({
                "function": provider.ready_check_js()
            }),
        );

        if element_res
            .and_then(|res| parse_script_result(&res))
            .map(|parsed| parsed.as_bool().unwrap_or(false))
            .unwrap_or(false)
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }

    if verbose {
        println!(
            "Warning: Timeout waiting for {} page elements. Proceeding anyway...",
            provider.display_name()
        );
    }
    Ok(())
}

fn open_url_tab(
    config_path: &str,
    provider: Provider,
    url: &str,
    headless: bool,
    verbose: bool,
) -> Result<(), String> {
    if verbose {
        println!("Opening URL: {}", url);
    }

    let list_res = call_mcp_tool(config_path, "list_pages", serde_json::json!({}))?;
    let text = list_res
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|obj| obj.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| format!("Invalid list_pages response structure: {:?}", list_res))?;

    let pages_before = parse_pages(text);
    let target_page_id = if pages_before.len() == 1
        && (pages_before[0].url == "about:blank"
            || pages_before[0].url.contains("new-tab-page")
            || pages_before[0].url.contains("chrome://welcome"))
    {
        call_mcp_tool(
            config_path,
            "navigate_page",
            serde_json::json!({
                "url": url
            }),
        )?;
        pages_before[0].id
    } else {
        call_mcp_tool(
            config_path,
            "new_page",
            serde_json::json!({
                "url": url
            }),
        )?;
        let refreshed_pages_res = call_mcp_tool(config_path, "list_pages", serde_json::json!({}))?;
        let refreshed_text = refreshed_pages_res
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|obj| obj.get("text"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| {
                format!(
                    "Invalid refreshed list_pages response structure: {:?}",
                    refreshed_pages_res
                )
            })?;
        let refreshed_pages = parse_pages(refreshed_text);
        unique_new_page_id(&pages_before, &refreshed_pages)?
    };

    call_mcp_tool(
        config_path,
        "select_page",
        serde_json::json!({
            "pageId": target_page_id,
            "bringToFront": !headless
        }),
    )?;

    let page_provider = Provider::from_url(url).unwrap_or(provider);
    wait_for_page_load(config_path, page_provider, verbose)
}

fn verify_resumed_session(
    config_path: &str,
    provider: Provider,
    requested_url: &str,
) -> Result<(), String> {
    if provider != Provider::M365Copilot {
        return Ok(());
    }

    let requested = Url::parse(requested_url)
        .map_err(|error| format!("session: invalid requested conversation URL: {error}"))?;
    let result = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": r#"() => ({
                href: window.location.href,
                authRedirect: /^(?:login\.microsoftonline\.com|login\.live\.com)$/i.test(window.location.hostname),
                composer: Boolean(document.querySelector('#m365-chat-editor-target-element')),
                conversationMessage: Boolean(document.querySelector(
                    '[data-testid="copilot-message-div"], [data-testid="m365-chat-llm-web-ui-chat-message"]'
                ))
            })"#
        }),
    )?;
    let parsed = parse_script_result(&result)?;
    if parsed
        .get("authRedirect")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return Err(
            "authentication: Microsoft sign-in interrupted session resume; rerun `ask-bridge --provider m365 login` headfully"
                .to_string(),
        );
    }

    let current = parsed
        .get("href")
        .and_then(|value| value.as_str())
        .and_then(|value| Url::parse(value).ok())
        .ok_or_else(|| "session: could not read the current conversation URL".to_string())?;
    let same_conversation = Provider::M365Copilot.owns_conversation_url(&current)
        && requested.host_str().map(str::to_ascii_lowercase)
            == current.host_str().map(str::to_ascii_lowercase)
        && requested.path() == current.path();
    let composer = parsed
        .get("composer")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let conversation_message = parsed
        .get("conversationMessage")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if !same_conversation || !composer || !conversation_message {
        return Err(
            "session: the requested Microsoft 365 conversation was not restored; the page returned to chat home, was unavailable, or lacked a conversation identity signal"
                .to_string(),
        );
    }

    Ok(())
}

fn copy_latest_markdown(config_path: &str, provider: Provider) -> Result<String, String> {
    let content = match copy_latest_markdown_via_clipboard(config_path, provider) {
        Ok(content) => Ok(content),
        Err(_) => scrape_latest_markdown_from_dom(config_path, provider),
    }?;

    validate_non_empty_response(provider, content)
}

fn allows_empty_markdown_for_explicit_image_output(
    provider: Provider,
    image_output: Option<&str>,
    error: &str,
) -> bool {
    provider == Provider::M365Copilot
        && image_output.is_some()
        && error.to_ascii_lowercase().contains("empty response")
}

fn copy_latest_markdown_for_request(
    config_path: &str,
    provider: Provider,
    image_output: Option<&str>,
) -> Result<String, String> {
    match copy_latest_markdown(config_path, provider) {
        Ok(markdown) => Ok(markdown),
        Err(error)
            if allows_empty_markdown_for_explicit_image_output(provider, image_output, &error) =>
        {
            Ok(String::new())
        }
        Err(error) => Err(error),
    }
}

fn validate_non_empty_response(provider: Provider, content: String) -> Result<String, String> {
    if content.trim().is_empty() {
        return Err(format!(
            "{} returned an empty response",
            provider.display_name()
        ));
    }

    Ok(content)
}

fn copy_latest_markdown_via_clipboard(
    config_path: &str,
    provider: Provider,
) -> Result<String, String> {
    let clipboard_before = read_clipboard().unwrap_or_default();
    let sentinel = format!("__ASK_CHATGPT_COPY_PENDING_{}__", std::process::id());
    write_clipboard(&sentinel)?;

    // Click the copy button, retrying if the message or button is not found yet (due to asynchronous rendering of Single Page App)
    let mut click_err = None;
    for _ in 0..30 {
        match click_latest_copy_button(config_path, provider) {
            Ok(_) => {
                click_err = None;
                break;
            }
            Err(e) => {
                click_err = Some(e);
                thread::sleep(Duration::from_millis(500));
            }
        }
    }

    if let Some(err) = click_err {
        // Restore clipboard before returning error
        let _ = write_clipboard(&clipboard_before);
        return Err(format!("Error copying latest response Markdown: {}", err));
    }

    let mut copied_content = None;
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(100));
        match read_clipboard() {
            Ok(content) if !content.trim().is_empty() && content != sentinel => {
                copied_content = Some(content);
                break;
            }
            _ => {}
        }
    }

    // Always restore the original clipboard
    let _ = write_clipboard(&clipboard_before);

    let content = copied_content
        .ok_or_else(|| "Timed out waiting for clipboard content after clicking copy".to_string())?;

    // Create a temporary file path
    let temp_path = std::env::temp_dir().join(format!("ask_chatgpt_{}.md", std::process::id()));

    // Write the copied content immediately to the temporary file
    std::fs::write(&temp_path, &content)
        .map_err(|e| format!("Failed to write to temporary file: {}", e))?;

    // Read the content back from the temporary file to output to the terminal
    let verified_content = std::fs::read_to_string(&temp_path)
        .map_err(|e| format!("Failed to read from temporary file: {}", e))?;

    // Clean up temporary file
    let _ = std::fs::remove_file(&temp_path);

    Ok(verified_content)
}

fn scrape_latest_markdown_from_dom(
    config_path: &str,
    provider: Provider,
) -> Result<String, String> {
    let latest_selector = serde_json::to_string(provider.latest_response_selector())
        .map_err(|e| format!("Failed to serialize response selector: {}", e))?;
    let content_selector = serde_json::to_string(provider.response_content_selector())
        .map_err(|e| format!("Failed to serialize response content selector: {}", e))?;
    let wait_for_m365_hydration = provider == Provider::M365Copilot;
    let inspect_js = r#"async () => {
        __M365_HELPER__
        const latestSelector = __LATEST_SELECTOR__;
        const contentSelector = __CONTENT_SELECTOR__;
        const waitForM365Hydration = __WAIT_FOR_M365_HYDRATION__;
        const findLatest = () => {
            const messages = Array.from(document.querySelectorAll(latestSelector))
                .filter((el) => ((el.innerText || el.textContent || '').trim().length > 0));
            return messages[messages.length - 1] || null;
        };
        const findTurn = () => {
            const latest = findLatest();
            return latest
                ? (contentSelector ? (latest.querySelector(contentSelector) || latest) : latest)
                : null;
        };

        if (waitForM365Hydration) {
            const sleep = (delay) => new Promise((resolve) => setTimeout(resolve, delay));
            let previousSignature = '';
            let stableSamples = 0;
            for (let attempt = 0; attempt < 100; attempt += 1) {
                const currentTurn = findTurn();
                const textLength = (currentTurn?.innerText || currentTurn?.textContent || '').trim().length;
                const codeBlocks = currentTurn
                    ? Array.from(currentTurn.querySelectorAll('.scriptor-component-code-block'))
                    : [];
                const hydratedCodeBlocks = codeBlocks.filter((block) => block.querySelector(
                    '[data-line-index], .view-line, [role="textbox"][aria-readonly="true"], ' +
                    '[role="textbox"][aria-multiline="true"], textarea, pre code'
                )).length;
                const signature = currentTurn
                    ? [
                        textLength,
                        currentTurn.childElementCount,
                        codeBlocks.length,
                        hydratedCodeBlocks,
                        currentTurn.querySelectorAll('[data-line-index], .view-line').length
                    ].join(':')
                    : '';

                stableSamples = signature && signature === previousSignature
                    ? stableSamples + 1
                    : 0;
                previousSignature = signature;

                const codeBlocksReady = codeBlocks.length > 0 &&
                    hydratedCodeBlocks === codeBlocks.length &&
                    stableSamples >= 3;
                const responseWithoutCodeIsStable = codeBlocks.length === 0 &&
                    textLength > 0 &&
                    stableSamples >= 30;
                if (codeBlocksReady || responseWithoutCodeIsStable) {
                    break;
                }
                await sleep(100);
            }
        }

        const latest = findLatest();
        if (!latest) return 'No assistant message found';
        const turn = findTurn();
        
        const elementToMarkdown = (element) => {
            let markdown = '';
            const processedSrcs = new Set();
            const walk = (node) => {
                if (node.nodeType === Node.TEXT_NODE) {
                    markdown += node.textContent;
                    return;
                }
                if (node.nodeType !== Node.ELEMENT_NODE) return;

                const tag = node.tagName.toLowerCase();
                
                const classText = Array.from(node.classList || []).join(' ');
                if (node.classList.contains('sr-only') ||
                    /screen-reader|visually-hidden|cdk-visually-hidden/.test(classText) ||
                    node.getAttribute('aria-hidden') === 'true' ||
                    tag === 'style' || tag === 'script') {
                    return;
                }

                // M365 citations are buttons with structured source URLs rather than anchors.
                if (tag === 'button' && node.hasAttribute('data-grouped-citations')) {
                    try {
                        const citations = JSON.parse(node.getAttribute('data-grouped-citations') || '[]');
                        const unique = new Set();
                        for (const citation of citations) {
                            const href = citation && citation.url ? String(citation.url) : '';
                            const label = citation && citation.name ? String(citation.name) : href;
                            if (!href || unique.has(href)) continue;
                            unique.add(href);
                            markdown += ' [' + label + '](' + href + ')';
                        }
                    } catch (_) {
                        const label = node.getAttribute('aria-label') || 'Citation';
                        markdown += ' [' + label + ']';
                    }
                    return;
                }

                if (tag === 'button') {
                    return;
                }

                // Code blocks
                if (tag === 'pre') {
                    const codeEl = node.querySelector('code');
                    const langClass = codeEl ? Array.from(codeEl.classList).find(c => c.startsWith('language-')) : '';
                    const lang = langClass ? langClass.replace('language-', '') : '';
                    const codeText = codeEl ? codeEl.textContent : node.textContent;
                    markdown += '\n```' + lang + '\n' + codeText + '\n```\n';
                    return;
                }

                if (node.classList.contains('scriptor-component-code-block')) {
                    const codeBlock = globalThis.AskBridgeM365Automation.extractCodeBlock(node);
                    markdown += '\n```' + codeBlock.language + '\n' + codeBlock.code + '\n```\n';
                    return;
                }

                // M365 renders fenced code in a read-only textbox instead of pre/code.
                if (node.getAttribute('role') === 'textbox' &&
                    (node.getAttribute('aria-readonly') === 'true' ||
                        node.getAttribute('aria-multiline') === 'true' ||
                        /code editor|程式碼編輯器|代码编辑器|コードエディター|코드 편집기/i.test(
                            node.getAttribute('aria-label') || ''
                        )) &&
                    !node.isContentEditable) {
                    const codeBlock = globalThis.AskBridgeM365Automation.extractCodeBlock(node);
                    markdown += '\n```' + codeBlock.language + '\n' + codeBlock.code + '\n```\n';
                    return;
                }

                // Inline code
                if (tag === 'code') {
                    if (!node.closest('pre')) {
                        markdown += '`' + node.textContent + '`';
                        return;
                    }
                }

                // Bold
                if (tag === 'strong' || tag === 'b') {
                    markdown += '**';
                    for (const child of node.childNodes) walk(child);
                    markdown += '**';
                    return;
                }

                // Italics
                if (tag === 'em' || tag === 'i') {
                    markdown += '*';
                    for (const child of node.childNodes) walk(child);
                    markdown += '*';
                    return;
                }

                // Links
                if (tag === 'a') {
                    const href = node.getAttribute('href') || '';
                    const text = node.textContent || '';
                    if (href && text) {
                        markdown += '[' + text + '](' + href + ')';
                        return;
                    }
                }

                const testId = node.getAttribute('data-testid') || '';
                if (/source-card|sources-button|chat-suggestion/i.test(testId)) {
                    return;
                }

                // Paragraphs, headers, list items
                if (tag === 'p') markdown += '\n';
                if (tag === 'br') markdown += '\n';
                if (tag === 'h1') markdown += '\n# ';
                if (tag === 'h2') markdown += '\n## ';
                if (tag === 'h3') markdown += '\n### ';
                if (tag === 'h4') markdown += '\n#### ';
                if (tag === 'h5') markdown += '\n##### ';
                if (tag === 'h6') markdown += '\n###### ';
                if (tag === 'li') markdown += '\n* ';

                // Images
                if (tag === 'img') {
                    const src = node.getAttribute('src') || '';
                    const alt = node.getAttribute('alt') || 'image';
                    if (src && !src.includes('avatar') && !src.includes('profile')) {
                        if (processedSrcs.has(src)) return;
                        processedSrcs.add(src);
                        markdown += '\n![' + alt + '](' + src + ')\n';
                        return;
                    }
                }

                for (const child of node.childNodes) {
                    walk(child);
                }

                if (['p', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'li'].includes(tag)) {
                    markdown += '\n';
                }
            };

            walk(element);
            return markdown.trim().replace(/\n{3,}/g, '\n\n');
        };
        
        return elementToMarkdown(turn);
    }"#
    .replace("__M365_HELPER__", include_str!("m365-automation.cjs"))
    .replace("__LATEST_SELECTOR__", &latest_selector)
    .replace("__CONTENT_SELECTOR__", &content_selector)
    .replace(
        "__WAIT_FOR_M365_HYDRATION__",
        if wait_for_m365_hydration {
            "true"
        } else {
            "false"
        },
    );

    let res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": inspect_js
        }),
    )?;

    let val = parse_script_result(&res)?;
    let content = val
        .as_str()
        .ok_or_else(|| "DOM scraper returned non-string result".to_string())?
        .to_string();

    if content == "No assistant message found" {
        return Err(format!(
            "No assistant message found on {} page",
            provider.display_name()
        ));
    }
    if content.trim().is_empty() {
        return Err(format!(
            "{} DOM scraper returned an empty response",
            provider.display_name()
        ));
    }

    Ok(content)
}

fn image_extension_from_bytes(
    bytes: &[u8],
    declared_type: Option<&str>,
) -> Result<&'static str, String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok("png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok("jpg");
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Ok("webp");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok("gif");
    }
    Err(format!(
        "image download: unsupported or invalid image content{}",
        declared_type
            .filter(|value| !value.is_empty())
            .map(|value| format!(" ({value})"))
            .unwrap_or_default()
    ))
}

fn unique_output_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("generated");
    let extension = path.extension().and_then(|value| value.to_str());
    for suffix in 2.. {
        let name = match extension {
            Some(extension) => format!("{stem}_{suffix}.{extension}"),
            None => format!("{stem}_{suffix}"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("an unused image output path should always be found")
}

fn explicit_image_output_path(
    output: &str,
    total: usize,
    index: usize,
    extension: &str,
) -> Result<PathBuf, String> {
    let path = Path::new(output);
    let is_directory = path.is_dir()
        || output.ends_with('/')
        || output.ends_with('\\')
        || path.extension().is_none();
    let target = if is_directory {
        std::fs::create_dir_all(path).map_err(|error| {
            format!("image download: failed to create output directory: {error}")
        })?;
        path.join(format!("generated_{}.{}", index + 1, extension))
    } else {
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!("image download: failed to create output directory: {error}")
            })?;
        }
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "image download: invalid output file name".to_string())?;
        let name = if total == 1 {
            format!("{stem}.{extension}")
        } else {
            format!("{stem}_{}.{}", index + 1, extension)
        };
        parent.join(name)
    };
    Ok(unique_output_path(target))
}

fn sanitize_m365_error_detail(detail: &str) -> String {
    detail
        .split_whitespace()
        .map(|token| {
            if token.starts_with("http://") || token.starts_with("https://") {
                "<url>"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}

fn interpret_m365_image_download_result(
    result: &Value,
) -> Result<(Vec<Value>, Vec<String>), String> {
    let status = result
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("error");
    if status == "error" {
        let detail = sanitize_m365_error_detail(
            result
                .get("error")
                .and_then(|value| value.as_str())
                .unwrap_or("browser image scan failed"),
        );
        let prefix = match classify_m365_error_message(&detail) {
            M365ErrorClass::Authentication => "authentication",
            M365ErrorClass::Policy => "policy",
            _ => "image download",
        };
        return Err(format!("{prefix}: {detail}"));
    }
    if status != "success" {
        return Err(format!(
            "image download: unexpected browser status '{status}'"
        ));
    }

    let images = result
        .get("images")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let failures: Vec<String> = result
        .get("failures")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .map(|failure| {
            sanitize_m365_error_detail(
                failure
                    .get("reason")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown browser-side failure"),
            )
        })
        .collect();

    if images.is_empty() {
        return Err(if failures.is_empty() {
            "image download: no generated image was found in the latest Microsoft 365 assistant turn"
                .to_string()
        } else {
            format!(
                "image download: all {} generated image candidate(s) failed: {}",
                failures.len(),
                failures.join("; ")
            )
        });
    }
    Ok((images, failures))
}

fn download_m365_images_from_latest_message(
    config_path: &str,
    image_output: Option<&str>,
    verbose: bool,
) -> Result<(), String> {
    let Some(image_output) = image_output else {
        return Ok(());
    };
    if verbose {
        println!("image download: scanning the latest Microsoft 365 assistant turn...");
    }

    let latest_selector = serde_json::to_string(Provider::M365Copilot.latest_response_selector())
        .map_err(|error| {
        format!("image download: failed to serialize response selector: {error}")
    })?;
    let helper = include_str!("m365-automation.cjs");
    let script = format!(
        r#"() => {{
            {helper}
            window.__ask_bridge_m365_image_download = {{ status: 'pending' }};
            (async () => {{
                const failures = [];
                try {{
                    const authRedirected = () => /^(?:login\.microsoftonline\.com|login\.live\.com)$/i.test(
                        window.location.hostname
                    );
                    if (authRedirected()) {{
                        throw new Error('authentication: Microsoft sign-in is required during image download');
                    }}
                    const turns = document.querySelectorAll({latest_selector});
                    const latestTurn = turns[turns.length - 1];
                    const candidates = globalThis.AskBridgeM365Automation.generatedImagesFromLatestTurn(latestTurn);
                    const images = [];
                    for (let index = 0; index < candidates.length; index += 1) {{
                        const candidate = candidates[index];
                        try {{
                            if (authRedirected()) {{
                                throw new Error('authentication: Microsoft sign-in expired during image download');
                            }}
                            let dataUrl = candidate.src;
                            let contentType = '';
                            if (!candidate.src.startsWith('data:image/')) {{
                                const response = await fetch(candidate.src, {{
                                    credentials: 'include',
                                    redirect: 'follow',
                                }});
                                if (authRedirected()) {{
                                    throw new Error('authentication: Microsoft sign-in expired during image download');
                                }}
                                if (!response.ok) throw new Error('HTTP ' + response.status);
                                contentType = response.headers.get('content-type') || '';
                                if (!contentType.toLowerCase().startsWith('image/')) {{
                                    throw new Error('unexpected content type ' + (contentType || '(missing)'));
                                }}
                                const blob = await response.blob();
                                dataUrl = await new Promise((resolve, reject) => {{
                                    const reader = new FileReader();
                                    reader.onloadend = () => resolve(reader.result);
                                    reader.onerror = () => reject(new Error('failed to read image bytes'));
                                    reader.readAsDataURL(blob);
                                }});
                            }}
                            if (!String(dataUrl || '').startsWith('data:image/')) {{
                                throw new Error('image source did not produce image bytes');
                            }}
                            images.push({{ index, dataUrl, contentType }});
                        }} catch (error) {{
                            failures.push({{ index, reason: error.message || String(error) }});
                        }}
                    }}
                    window.__ask_bridge_m365_image_download = {{
                        status: 'success',
                        images,
                        failures,
                    }};
                }} catch (error) {{
                    window.__ask_bridge_m365_image_download = {{
                        status: 'error',
                        error: error.message || String(error),
                        images: [],
                        failures,
                    }};
                }}
            }})();
            return true;
        }}"#
    );
    let started = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({ "function": script }),
    )?;
    if !parse_script_result(&started)?.as_bool().unwrap_or(false) {
        return Err("image download: failed to start browser image scan".to_string());
    }

    let mut result = None;
    for _ in 0..150 {
        thread::sleep(Duration::from_millis(100));
        let status = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({
                "function": "() => window.__ask_bridge_m365_image_download || { status: 'pending' }"
            }),
        )?;
        let parsed = parse_script_result(&status)?;
        if parsed.get("status").and_then(|value| value.as_str()) != Some("pending") {
            result = Some(parsed);
            break;
        }
    }
    let result = result
        .ok_or_else(|| "image download: timed out before any files were written".to_string())?;
    let (images, failures) = interpret_m365_image_download_result(&result)?;

    let mut saved = Vec::new();
    for (index, image) in images.iter().enumerate() {
        let data_url = image
            .get("dataUrl")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                "image download: browser returned an invalid image payload".to_string()
            })?;
        let (header, encoded) = data_url
            .split_once(',')
            .ok_or_else(|| "image download: browser returned a malformed data URL".to_string())?;
        let bytes = general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| format!("image download: failed to decode image bytes: {error}"))?;
        let extension = image_extension_from_bytes(
            &bytes,
            image
                .get("contentType")
                .and_then(|value| value.as_str())
                .or(Some(header)),
        )?;
        let path = explicit_image_output_path(image_output, images.len(), index, extension)?;
        std::fs::write(&path, bytes)
            .map_err(|error| format!("image download: failed to write output file: {error}"))?;
        println!("Downloaded generated image to: {}", path.to_string_lossy());
        saved.push(path);
    }

    if !failures.is_empty() {
        return Err(format!(
            "image download: saved {} image(s), but {} candidate(s) failed: {}",
            saved.len(),
            failures.len(),
            failures.join("; ")
        ));
    }
    Ok(())
}

fn download_images_from_latest_message(
    config_path: &str,
    provider: Provider,
    image_output: Option<&str>,
    verbose: bool,
) -> Result<(), String> {
    if provider == Provider::M365Copilot {
        return download_m365_images_from_latest_message(config_path, image_output, verbose);
    }
    if verbose {
        println!("Checking for generated images in the latest assistant response...");
    }
    let latest_selector = serde_json::to_string(provider.latest_response_selector())
        .map_err(|e| format!("Failed to serialize response selector: {}", e))?;
    let image_scan_js = r#"() => {
                window.__downloaded_images_status = "pending";
                window.__downloaded_images = null;
                (async () => {
                    try {
                        const messages = document.querySelectorAll(__LATEST_SELECTOR__);
                        const latestMessage = messages[messages.length - 1];
                        if (!latestMessage) {
                            window.__downloaded_images = [];
                            window.__downloaded_images_status = "success";
                            return;
                        }
                        
                        const imgs = Array.from(latestMessage.querySelectorAll('img'));
                        const seenSrcs = new Set();
                        const candidateImgs = imgs.filter(img => {
                            const src = img.src || '';
                            if (src.includes('avatar') || src.includes('profile')) return false;
                            const width = img.naturalWidth || img.width || 0;
                            const height = img.naturalHeight || img.height || 0;
                            if (width > 0 && width < 100) return false;
                            if (height > 0 && height < 100) return false;
                            if (!src.startsWith('http') && !src.startsWith('blob:') && !src.startsWith('data:image/')) return false;
                            if (seenSrcs.has(src)) return false;
                            seenSrcs.add(src);
                            return true;
                        });

                        const imagesData = [];
                        for (let i = 0; i < candidateImgs.length; i++) {
                            const img = candidateImgs[i];
                            try {
                                if (!img.complete) {
                                    await new Promise((resolve) => {
                                        img.addEventListener('load', resolve);
                                        img.addEventListener('error', resolve);
                                        setTimeout(resolve, 10000);
                                    });
                                }

                                let dataUrl = "";
                                if ((img.src || '').startsWith('data:image/')) {
                                    dataUrl = img.src;
                                } else {
                                    try {
                                        const response = await fetch(img.src);
                                        const blob = await response.blob();
                                        dataUrl = await new Promise((resolve, reject) => {
                                            const reader = new FileReader();
                                            reader.onloadend = () => resolve(reader.result);
                                            reader.onerror = reject;
                                            reader.readAsDataURL(blob);
                                        });
                                    } catch (fetchErr) {
                                        const canvas = document.createElement('canvas');
                                        canvas.width = img.naturalWidth || img.width || 512;
                                        canvas.height = img.naturalHeight || img.height || 512;
                                        const ctx = canvas.getContext('2d');
                                        ctx.drawImage(img, 0, 0);
                                        dataUrl = canvas.toDataURL('image/png');
                                    }
                                }

                                if (dataUrl && dataUrl.startsWith('data:image/')) {
                                    imagesData.push({
                                        index: i,
                                        src: img.src,
                                        alt: img.alt || "",
                                        dataUrl: dataUrl
                                    });
                                }
                            } catch (err) {
                                // ignore
                            }
                        }
                        window.__downloaded_images = imagesData;
                        window.__downloaded_images_status = "success";
                    } catch (e) {
                        window.__downloaded_images_status = "error: " + e.message;
                    }
                })();
                return { ok: true };
            }"#
    .replace("__LATEST_SELECTOR__", &latest_selector);

    let start_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": image_scan_js
        }),
    )?;

    let start_parsed = parse_script_result(&start_res)?;
    if !start_parsed["ok"].as_bool().unwrap_or(false) {
        return Err("Failed to initiate image scanning script".to_string());
    }

    let mut wait_cycles = 0;
    let mut status = String::from("pending");
    while status == "pending" && wait_cycles < 150 {
        thread::sleep(Duration::from_millis(100));
        let check_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({
                "function": "() => window.__downloaded_images_status || 'pending'"
            }),
        )?;
        if let Some(s) = parse_script_result(&check_res)
            .ok()
            .and_then(|p| p.as_str().map(|str_ref| str_ref.to_string()))
        {
            status = s;
        }
        wait_cycles += 1;
    }

    if status.starts_with("error:") {
        return Err(format!("Image scanning failed: {}", status));
    }

    if status == "pending" {
        return Err("Timed out waiting for images to download in browser".to_string());
    }

    let get_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": r#"() => {
                const res = window.__downloaded_images || [];
                delete window.__downloaded_images;
                delete window.__downloaded_images_status;
                return res;
            }"#
        }),
    )?;

    let parsed = parse_script_result(&get_res)?;
    let images = match parsed.as_array() {
        Some(arr) => arr,
        None => return Ok(()),
    };

    if images.is_empty() {
        if verbose {
            println!("No generated images found in the latest response.");
        }
        return Ok(());
    }

    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let total = images.len();
    for (idx, img) in images.iter().enumerate() {
        let data_url = match img["dataUrl"].as_str() {
            Some(s) => s,
            None => continue,
        };

        let parts: Vec<&str> = data_url.splitn(2, ',').collect();
        if parts.len() != 2 {
            continue;
        }

        let header = parts[0];
        let base64_data = parts[1];

        let ext = if header.contains("image/png") {
            "png"
        } else if header.contains("image/jpeg") || header.contains("image/jpg") {
            "jpg"
        } else if header.contains("image/webp") {
            "webp"
        } else {
            "png"
        };

        let decoded = general_purpose::STANDARD
            .decode(base64_data)
            .map_err(|e| format!("Failed to decode base64 data: {}", e))?;

        let file_path = match image_output {
            Some(output_str) => {
                let path = std::path::Path::new(output_str);
                let is_dir = path.is_dir()
                    || output_str.ends_with('/')
                    || output_str.ends_with('\\')
                    || path.extension().is_none();

                if is_dir {
                    std::fs::create_dir_all(path)
                        .map_err(|e| format!("Failed to create directory {:?}: {}", path, e))?;
                    path.join(format!("generated_{}_{}.{}", epoch, idx, ext))
                } else {
                    let parent = path.parent().unwrap_or_else(|| std::path::Path::new(""));
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            format!("Failed to create parent directory {:?}: {}", parent, e)
                        })?;
                    }
                    let file_stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .ok_or_else(|| "Invalid file name".to_string())?;
                    let file_ext = path.extension().and_then(|e| e.to_str()).unwrap_or(ext);

                    if total <= 1 {
                        parent.join(format!("{}.{}", file_stem, file_ext))
                    } else {
                        parent.join(format!("{}_{}.{}", file_stem, idx + 1, file_ext))
                    }
                }
            }
            None => {
                std::fs::create_dir_all("target")
                    .map_err(|e| format!("Failed to create target/ directory: {}", e))?;
                std::path::PathBuf::from(format!("target/generated_{}_{}.{}", epoch, idx, ext))
            }
        };

        std::fs::write(&file_path, decoded)
            .map_err(|e| format!("Failed to write image file {:?}: {}", file_path, e))?;

        println!(
            "Downloaded and saved generated image to: {}",
            file_path.to_string_lossy()
        );
    }

    Ok(())
}

/// Display an image in the terminal using kitty's icat protocol.
/// Silently skips if kitty icat is not available.
fn display_image_in_terminal(image_path: &str) {
    let _ = Command::new("kitty").args(["icat", image_path]).status();
}

fn wait_for_attachment_indicator(
    config_path: &str,
    provider: Provider,
    path: &str,
    verbose: bool,
) -> Result<(), String> {
    let file_name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);
    let file_stem = Path::new(path)
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or(file_name);
    let file_name_json = serde_json::to_string(file_name)
        .map_err(|e| format!("Failed to serialize file name: {}", e))?;
    let file_stem_json = serde_json::to_string(file_stem)
        .map_err(|e| format!("Failed to serialize file stem: {}", e))?;
    let js = r#"() => {
        const fileName = __FILE_NAME__;
        const fileStem = __FILE_STEM__;
        const text = document.body.innerText || '';
        return text.includes(fileName) || text.includes(fileStem);
    }"#
    .replace("__FILE_NAME__", &file_name_json)
    .replace("__FILE_STEM__", &file_stem_json);

    for _ in 0..30 {
        let check_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({ "function": js }),
        )?;
        if parse_script_result(&check_res)
            .ok()
            .and_then(|p| p.as_bool())
            .unwrap_or(false)
        {
            if verbose {
                println!(
                    "{} accepted attachment '{}'",
                    provider.display_name(),
                    file_name
                );
            }
            return Ok(());
        }
        thread::sleep(Duration::from_millis(500));
    }

    Err(format!(
        "Timed out waiting for {} to show attachment '{}'",
        provider.display_name(),
        file_name
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttachmentKind {
    Image,
    File,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum M365ErrorClass {
    Authentication,
    Policy,
    Selector,
    Timeout,
    Rejected,
    Unknown,
}

fn classify_m365_error_message(message: &str) -> M365ErrorClass {
    let normalized = message.to_lowercase();
    if normalized.contains("authentication")
        || normalized.contains("sign-in")
        || normalized.contains("sign in")
        || normalized.contains("login.microsoftonline.com")
    {
        M365ErrorClass::Authentication
    } else if normalized.contains("dlp")
        || normalized.contains("policy")
        || normalized.contains("organization")
        || normalized.contains("blocked")
        || normalized.contains("conditional access")
    {
        M365ErrorClass::Policy
    } else if normalized.contains("selector")
        || normalized.contains("control not found")
        || normalized.contains("picker not found")
        || normalized.contains("ui rollout")
    {
        M365ErrorClass::Selector
    } else if normalized.contains("timeout") || normalized.contains("timed out") {
        M365ErrorClass::Timeout
    } else if normalized.contains("rejected")
        || normalized.contains("unsupported")
        || normalized.contains("failed")
        || normalized.contains("error")
    {
        M365ErrorClass::Rejected
    } else {
        M365ErrorClass::Unknown
    }
}

impl AttachmentKind {
    fn stage_name(self) -> &'static str {
        match self {
            AttachmentKind::Image => "image upload",
            AttachmentKind::File => "file upload",
        }
    }
}

fn attachment_basename(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<attachment>")
}

fn m365_image_extension_allowed(extension: &str) -> bool {
    matches!(extension, "png" | "jpg" | "jpeg")
}

fn validate_m365_file_signature(
    kind: AttachmentKind,
    extension: &str,
    header: &[u8],
) -> Result<(), String> {
    let valid = match (kind, extension) {
        (AttachmentKind::Image, "png") => header.starts_with(b"\x89PNG\r\n\x1a\n"),
        (AttachmentKind::Image, "jpg" | "jpeg") => header.starts_with(&[0xff, 0xd8, 0xff]),
        (AttachmentKind::File, "pdf") => header.starts_with(b"%PDF-"),
        (AttachmentKind::File, "docx") => header.starts_with(b"PK"),
        (AttachmentKind::File, "txt") => true,
        (AttachmentKind::File, _) => return Ok(()),
        (AttachmentKind::Image, _) => false,
    };
    valid.then_some(()).ok_or_else(|| {
        format!(
            "{}: attachment content does not match .{extension}",
            kind.stage_name()
        )
    })
}

fn validate_attachment_path(
    provider: Provider,
    kind: AttachmentKind,
    path: &str,
) -> Result<(), String> {
    let basename = attachment_basename(path);
    if path.trim().is_empty() {
        return Err(format!(
            "{}: attachment path cannot be empty",
            kind.stage_name()
        ));
    }
    let metadata = std::fs::metadata(path).map_err(|error| {
        format!(
            "{}: cannot access '{}': {error}",
            kind.stage_name(),
            basename
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "{}: '{}' is not a regular file",
            kind.stage_name(),
            basename
        ));
    }

    if provider != Provider::M365Copilot {
        return Ok(());
    }

    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if kind == AttachmentKind::Image && !m365_image_extension_allowed(&extension) {
        return Err(format!(
            "{}: unsupported extension for '{}'. Microsoft 365 Copilot V2 supports PNG and JPEG images.",
            kind.stage_name(),
            basename
        ));
    }
    if kind == AttachmentKind::Image && metadata.len() == 0 {
        return Err(format!("{}: '{}' is empty", kind.stage_name(), basename));
    }

    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("{}: cannot read '{}': {error}", kind.stage_name(), basename))?;
    let mut header = [0u8; 8];
    let bytes_read = file
        .read(&mut header)
        .map_err(|error| format!("{}: cannot read '{}': {error}", kind.stage_name(), basename))?;
    validate_m365_file_signature(kind, &extension, &header[..bytes_read])
        .map_err(|error| format!("{error} ('{basename}')"))
}

fn validate_attachment_inputs(
    provider: Provider,
    image_paths: &[String],
    file_paths: &[String],
) -> Result<(), String> {
    for path in image_paths {
        validate_attachment_path(provider, AttachmentKind::Image, path)?;
    }
    for path in file_paths {
        validate_attachment_path(provider, AttachmentKind::File, path)?;
    }
    Ok(())
}

fn accept_rule_matches(accept: &str, file_name: &str, mime: &str) -> bool {
    let file_name = file_name.to_ascii_lowercase();
    let mime = mime.to_ascii_lowercase();
    let top_level = mime.split('/').next().unwrap_or("");
    let rules: Vec<String> = accept
        .split(',')
        .map(|rule| rule.trim().to_ascii_lowercase())
        .filter(|rule| !rule.is_empty())
        .collect();
    rules.is_empty()
        || rules.iter().any(|rule| {
            rule == "*/*"
                || rule == &mime
                || (rule.starts_with('.') && file_name.ends_with(rule))
                || (rule.ends_with("/*")
                    && !top_level.is_empty()
                    && rule == &format!("{top_level}/*"))
        })
}

fn wait_for_m365_attachment(
    config_path: &str,
    path: &str,
    kind: AttachmentKind,
    verbose: bool,
) -> Result<(), String> {
    let file_name = attachment_basename(path);
    let file_name_json = serde_json::to_string(file_name).map_err(|error| {
        format!(
            "{}: failed to serialize attachment name: {error}",
            kind.stage_name()
        )
    })?;
    let helper = include_str!("m365-automation.cjs");
    let script = format!(
        r#"() => {{
            {helper}
            return globalThis.AskBridgeM365Automation.inspectAttachment({file_name_json});
        }}"#
    );

    for _ in 0..120 {
        let result = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({ "function": script }),
        )?;
        let parsed = parse_script_result(&result)?;
        match parsed
            .get("status")
            .and_then(|status| status.as_str())
            .unwrap_or("not-found")
        {
            "done" => {
                if verbose {
                    println!(
                        "{}: Microsoft 365 Copilot accepted '{}'",
                        kind.stage_name(),
                        file_name
                    );
                }
                return Ok(());
            }
            "policy" => {
                let detail = parsed
                    .get("detail")
                    .and_then(|detail| detail.as_str())
                    .unwrap_or("organization policy blocked the attachment");
                return Err(format!("policy: {detail}"));
            }
            "authentication" => {
                return Err(
                    "authentication: Microsoft sign-in expired during attachment upload; rerun `ask-bridge --provider m365 login` headfully"
                        .to_string(),
                );
            }
            "error" => {
                let detail = parsed
                    .get("detail")
                    .and_then(|detail| detail.as_str())
                    .unwrap_or("attachment rejected");
                return Err(format!("{}: {detail}", kind.stage_name()));
            }
            _ => thread::sleep(Duration::from_millis(500)),
        }
    }

    Err(format!(
        "{}: upload timed out for '{}'; the prompt was not submitted",
        kind.stage_name(),
        file_name
    ))
}

fn upload_m365_attachment(
    config_path: &str,
    path: &str,
    kind: AttachmentKind,
    verbose: bool,
) -> Result<(), String> {
    let canonical_path = std::fs::canonicalize(path).map_err(|error| {
        format!(
            "{}: cannot resolve '{}': {error}",
            kind.stage_name(),
            attachment_basename(path)
        )
    })?;
    let canonical_path = canonical_path.to_string_lossy().to_string();
    let open_menu_script = r#"async () => {
        const isVisible = (element) => {
            if (!element) return false;
            if (typeof element.getClientRects !== 'function') return true;
            return element.getClientRects().length > 0;
        };
        let button;
        for (let attempt = 0; attempt < 20; attempt += 1) {
            const scope = document.querySelector('#m365-chat-input-shared-container');
            button = Array.from(
                scope?.querySelectorAll('[data-testid="PlusMenuButton"]') || []
            ).find(isVisible);
            if (button) break;
            await new Promise((resolve) => setTimeout(resolve, 250));
        }
        if (!button) {
            return { ok: false, error: 'upload control not found; this may be a Microsoft 365 UI rollout' };
        }
        if (button.disabled || button.getAttribute('aria-disabled') === 'true') {
            return { ok: false, error: 'upload control is locked or unavailable' };
        }
        button.click();
        const input = document.querySelector('input[type="file"]');
        return { ok: true, accept: input?.getAttribute('accept') || '' };
    }"#;
    let open_result = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({ "function": open_menu_script }),
    )?;
    let open_parsed = parse_script_result(&open_result)?;
    if !open_parsed
        .get("ok")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return Err(format!(
            "{}: {}",
            kind.stage_name(),
            open_parsed
                .get("error")
                .and_then(|value| value.as_str())
                .unwrap_or("upload control not found; this may be a Microsoft 365 UI rollout")
        ));
    }

    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = mime_type_for_extension(&extension);
    let accept = open_parsed
        .get("accept")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if !accept_rule_matches(accept, attachment_basename(path), mime) {
        return Err(format!(
            "{}: Microsoft 365 upload input does not accept '{}'",
            kind.stage_name(),
            attachment_basename(path)
        ));
    }

    thread::sleep(Duration::from_millis(500));
    let snapshot = take_snapshot_text(config_path)?;
    let upload_uid = find_snapshot_uid(&snapshot, &["upload", "images", "files"], &["cloud"])
        .or_else(|| find_snapshot_uid(&snapshot, &["上傳", "圖片", "檔案"], &["雲端"]))
        .or_else(|| find_snapshot_uid(&snapshot, &["上傳", "影像", "檔案"], &["雲端"]))
        .or_else(|| find_snapshot_uid(&snapshot, &["上传", "图片", "文件"], &["云端"]))
        .ok_or_else(|| {
            format!(
                "{}: upload menu item not found; this may be a Microsoft 365 UI rollout",
                kind.stage_name()
            )
        })?;

    if verbose {
        println!(
            "{}: uploading '{}' to Microsoft 365 Copilot...",
            kind.stage_name(),
            attachment_basename(path)
        );
    }
    call_mcp_tool(
        config_path,
        "upload_file",
        serde_json::json!({
            "uid": upload_uid,
            "filePath": canonical_path,
            "includeSnapshot": false
        }),
    )
    .map_err(|error| format!("{}: upload_file failed: {error}", kind.stage_name()))?;
    wait_for_m365_attachment(config_path, path, kind, verbose)
}

fn upload_attachments_via_file_chooser(
    config_path: &str,
    provider: Provider,
    image_paths: &[String],
    file_paths: &[String],
    verbose: bool,
) -> Result<(), String> {
    if provider == Provider::M365Copilot {
        for path in image_paths {
            upload_m365_attachment(config_path, path, AttachmentKind::Image, verbose)?;
        }
        for path in file_paths {
            upload_m365_attachment(config_path, path, AttachmentKind::File, verbose)?;
        }
        return Ok(());
    }

    for (path, verify_filename) in image_paths
        .iter()
        .map(|path| (path, false))
        .chain(file_paths.iter().map(|path| (path, true)))
    {
        let canonical_path = std::fs::canonicalize(path)
            .map_err(|e| format!("Failed to resolve file '{}': {}", path, e))?;
        let file_path = canonical_path.to_string_lossy().to_string();

        let snapshot = take_snapshot_text(config_path)?;
        let menu_uid = match provider {
            Provider::Gemini => {
                find_snapshot_uid(&snapshot, &["上傳與工具"], &["更多", "雲端", "drive"])
                    .or_else(|| find_snapshot_uid(&snapshot, &["upload"], &["drive"]))
            }
            Provider::ChatGpt => find_snapshot_uid(&snapshot, &["attach"], &["settings", "menu"]),
            Provider::Claude => find_snapshot_uid(&snapshot, &["attach"], &["settings", "menu"])
                .or_else(|| find_snapshot_uid(&snapshot, &["upload"], &["drive"])),
            Provider::M365Copilot => None,
        }
        .ok_or_else(|| {
            format!(
                "Could not find {} upload menu in page snapshot",
                provider.display_name()
            )
        })?;

        call_mcp_tool(
            config_path,
            "click",
            serde_json::json!({
                "uid": menu_uid,
                "includeSnapshot": false
            }),
        )?;
        thread::sleep(Duration::from_millis(500));

        let snapshot = take_snapshot_text(config_path)?;
        let upload_uid = match provider {
            Provider::Gemini => find_snapshot_uid(&snapshot, &["上傳檔案"], &["雲端", "drive"])
                .or_else(|| find_snapshot_uid(&snapshot, &["upload", "file"], &["drive"])),
            Provider::ChatGpt => find_snapshot_uid(&snapshot, &["file"], &["drive", "connect"]),
            Provider::Claude => {
                find_snapshot_uid(&snapshot, &["upload", "file"], &["drive", "connect"])
                    .or_else(|| find_snapshot_uid(&snapshot, &["file"], &["drive", "connect"]))
            }
            Provider::M365Copilot => None,
        }
        .unwrap_or_else(|| menu_uid.clone());

        if verbose {
            println!(
                "Uploading attachment '{}' to {}...",
                attachment_basename(path),
                provider.display_name()
            );
        }
        call_mcp_tool(
            config_path,
            "upload_file",
            serde_json::json!({
                "uid": upload_uid,
                "filePath": file_path,
                "includeSnapshot": false
            }),
        )?;
        if verify_filename {
            wait_for_attachment_indicator(config_path, provider, path, verbose)?;
        } else {
            thread::sleep(Duration::from_millis(800));
        }
    }

    Ok(())
}

/// Map a file extension to a MIME type. Covers common image and document formats.
/// `ext` is expected to already be lowercased by the caller.
fn mime_type_for_extension(ext: &str) -> &'static str {
    match ext {
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        // Documents
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "rtf" => "application/rtf",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "json" => "application/json",
        "yaml" | "yml" => "text/yaml",
        "ts" => "text/typescript",
        "tsx" => "text/typescript",
        "js" | "mjs" | "cjs" => "text/javascript",
        "jsx" => "text/javascript",
        "css" => "text/css",
        "py" => "text/x-python",
        "rb" => "text/x-ruby",
        "go" => "text/x-go",
        "rs" => "text/x-rust",
        "java" => "text/x-java",
        "kt" => "text/x-kotlin",
        "c" => "text/x-c",
        "h" => "text/x-c",
        "cpp" | "cc" | "cxx" => "text/x-c++",
        "hpp" => "text/x-c++",
        "cs" => "text/x-csharp",
        "swift" => "text/x-swift",
        "php" => "text/x-php",
        "sh" => "application/x-sh",
        "bash" => "application/x-sh",
        "zsh" => "application/x-sh",
        "sql" => "application/sql",
        "toml" => "application/toml",
        "ini" => "text/plain",
        "log" => "text/plain",
        // Archives
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "tar" => "application/x-tar",
        "bz2" => "application/x-bzip2",
        "7z" => "application/x-7z-compressed",
        // Audio
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        // Video
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// Upload local image and/or document files to the provider prompt composer using the
/// best available provider-specific upload mechanism.
/// Returns an error string if any attachment fails to upload.
fn upload_attachments_to_provider(
    config_path: &str,
    provider: Provider,
    image_paths: &[String],
    file_paths: &[String],
    verbose: bool,
) -> Result<(), String> {
    let total = image_paths.len() + file_paths.len();
    if total == 0 {
        return Ok(());
    }

    if provider == Provider::M365Copilot {
        return upload_attachments_via_file_chooser(
            config_path,
            provider,
            image_paths,
            file_paths,
            verbose,
        );
    }

    let data_transfer_image_paths: &[String] = if provider == Provider::Gemini
        && !image_paths.is_empty()
    {
        match upload_attachments_via_file_chooser(config_path, provider, image_paths, &[], verbose)
        {
            Ok(()) => &[],
            Err(e) => {
                if verbose {
                    eprintln!(
                        "Warning: {} image file chooser upload failed, trying DataTransfer fallback: {}",
                        provider.display_name(),
                        e
                    );
                }
                image_paths
            }
        }
    } else {
        image_paths
    };

    let data_transfer_total = data_transfer_image_paths.len() + file_paths.len();
    if data_transfer_total == 0 {
        return Ok(());
    }

    if verbose {
        println!(
            "Attaching {} attachment(s) ({} image(s), {} file(s)) to the prompt...",
            data_transfer_total,
            data_transfer_image_paths.len(),
            file_paths.len()
        );
    }

    // Build a JSON array of { name, mime, base64 } objects. Images first, then other files.
    // We pass raw base64 + mime and decode in JS to avoid `fetch(data:...)` which ChatGPT's
    // Content-Security-Policy blocks (results in "Failed to fetch").
    let mut files_json = Vec::new();
    for path in data_transfer_image_paths.iter().chain(file_paths.iter()) {
        let bytes =
            std::fs::read(path).map_err(|e| format!("Failed to read file '{}': {}", path, e))?;
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let mime = mime_type_for_extension(&ext);
        let b64 = general_purpose::STANDARD.encode(&bytes);
        let file_name = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment")
            .to_string();
        files_json.push(serde_json::json!({
            "name": file_name,
            "mime": mime,
            "base64": b64
        }));
    }

    let files_json_str = serde_json::to_string(&files_json)
        .map_err(|e| format!("Failed to serialize attachment data: {}", e))?;
    let composer_selectors = provider.composer_selectors_json();
    // Build JS without raw strings to avoid r#"..."# termination conflicts
    let js = "() => {\n".to_string()
        + "    window.__upload_images_status = 'pending';\n"
        + "    (async () => {\n"
        + "        try {\n"
        + &format!("            const filesData = {};\n", files_json_str)
        + "            const decodeB64 = (b64) => {\n"
        + "                const bin = atob(b64);\n"
        + "                const len = bin.length;\n"
        + "                const bytes = new Uint8Array(len);\n"
        + "                for (let i = 0; i < len; i++) bytes[i] = bin.charCodeAt(i);\n"
        + "                return bytes;\n"
        + "            };\n"
        + "            const fileObjects = filesData.map((f) => {\n"
        + "                const bytes = decodeB64(f.base64);\n"
        + "                const blob = new Blob([bytes], { type: f.mime || 'application/octet-stream' });\n"
        + "                return new File([blob], f.name, { type: blob.type });\n"
        + "            });\n"
        + &format!(
            "            const composerSelectors = {};\n",
            composer_selectors
        )
        + "            const el = composerSelectors.map((s) => document.querySelector(s)).find(Boolean);\n"
        + "            if (!el) {\n"
        + "                window.__upload_images_status = 'error: composer not found';\n"
        + "                return;\n"
        + "            }\n"
        + "            el.focus();\n"
        + "            const fileInputs = Array.from(document.querySelectorAll('input[type=\"file\"]'));\n"
        + "            // Pick the file input whose `accept` attribute covers every attached file.\n"
        + "            // An input accepts a file when accept is empty, contains `*/*` or a matching\n"
        + "            // wildcard (e.g. `image/*`), or lists the file's exact MIME type.\n"
        + "            const accepts = (input, file) => {\n"
        + "                const acc = (input.getAttribute('accept') || '').trim();\n"
        + "                if (!acc) return true;\n"
        + "                const parts = acc.split(',').map(s => s.trim().toLowerCase()).filter(Boolean);\n"
        + "                const mime = (file.type || '').toLowerCase();\n"
        + "                const top = mime.split('/')[0];\n"
        + "                const name = (file.name || '').toLowerCase();\n"
        + "                return parts.some(p => p === '*/*' || p === mime || (p.startsWith('.') && name.endsWith(p)) || (p.endsWith('/*') && top && p === top + '/*'));\n"
        + "            };\n"
        + "            const fileInput = fileInputs.find(i => fileObjects.every(f => accepts(i, f)))\n"
        + "                || fileInputs.find(i => !i.getAttribute('accept'))\n"
        + "                || fileInputs[0];\n"
        + "            if (fileInput) {\n"
        + "                const dt = new DataTransfer();\n"
        + "                for (const f of fileObjects) dt.items.add(f);\n"
        + "                fileInput.files = dt.files;\n"
        + "                fileInput.dispatchEvent(new Event('change', { bubbles: true }));\n"
        + "                window.__upload_images_status = 'success:file-input';\n"
        + "                return;\n"
        + "            }\n"
        + "            const dt = new DataTransfer();\n"
        + "            for (const f of fileObjects) dt.items.add(f);\n"
        + "            const targets = [el, el.closest('form'), document.querySelector('main'), document.body].filter(Boolean);\n"
        + "            for (const target of targets) {\n"
        + "                for (const type of ['dragenter', 'dragover', 'drop']) {\n"
        + "                    target.dispatchEvent(new DragEvent(type, {\n"
        + "                        bubbles: true, cancelable: true, dataTransfer: dt\n"
        + "                    }));\n"
        + "                }\n"
        + "            }\n"
        + "            const pasteEvent = new ClipboardEvent('paste', {\n"
        + "                bubbles: true, cancelable: true, clipboardData: dt\n"
        + "            });\n"
        + "            el.dispatchEvent(pasteEvent);\n"
        + "            window.__upload_images_status = 'success:drop';\n"
        + "        } catch (e) {\n"
        + "            window.__upload_images_status = 'error: ' + e.message;\n"
        + "        }\n"
        + "    })();\n"
        + "    return true;\n"
        + "}";

    let start_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({ "function": js }),
    )?;

    let start_parsed = parse_script_result(&start_res)?;
    if !start_parsed.as_bool().unwrap_or(false) {
        return Err("Failed to initiate attachment upload script".to_string());
    }

    // Poll for completion. Allow up to ~60s for large document uploads.
    let mut wait_cycles = 0;
    let mut status = String::from("pending");
    while status == "pending" && wait_cycles < 300 {
        thread::sleep(Duration::from_millis(200));
        let check_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({ "function": "() => window.__upload_images_status || 'pending'" }),
        )?;
        if let Some(s) = parse_script_result(&check_res)
            .ok()
            .and_then(|p| p.as_str().map(|r| r.to_string()))
        {
            status = s;
        }
        wait_cycles += 1;
    }

    if status.starts_with("error:") {
        return Err(format!("Attachment upload failed: {}", status));
    }
    if status == "pending" {
        return Err("Timed out waiting for attachments to upload".to_string());
    }

    if verbose {
        println!("Attachments attached successfully ({})", status);
    }

    // Give the UI a moment to render the attachments before typing the prompt
    thread::sleep(Duration::from_millis(800));

    if provider == Provider::Gemini {
        // Gemini renders image attachments as thumbnails without a stable filename in
        // the accessible text. Text/document chips do expose their filename, so keep
        // the stricter post-upload check for `--file` attachments only.
        for path in file_paths {
            if let Err(e) = wait_for_attachment_indicator(config_path, provider, path, verbose) {
                if verbose {
                    eprintln!(
                        "Warning: {} DataTransfer upload was not detected, trying file chooser fallback: {}",
                        provider.display_name(),
                        e
                    );
                }
                return upload_attachments_via_file_chooser(
                    config_path,
                    provider,
                    image_paths,
                    file_paths,
                    verbose,
                );
            }
        }
    }

    Ok(())
}

#[derive(Clone, Copy)]
enum SelectionKind {
    Model,
    Reasoning,
}

impl SelectionKind {
    fn display_name(self) -> &'static str {
        match self {
            SelectionKind::Model => "model",
            SelectionKind::Reasoning => "reasoning",
        }
    }

    fn stage_name(self) -> &'static str {
        match self {
            SelectionKind::Model => "model selection",
            SelectionKind::Reasoning => "reasoning selection",
        }
    }
}

fn interpret_selection_status(kind: SelectionKind, status: &str) -> Result<&str, String> {
    if status == "pending" {
        return Err(format!(
            "{}: timed out waiting for selected state verification",
            kind.stage_name()
        ));
    }
    if let Some(error) = status.strip_prefix("error:") {
        return Err(format!("{}: {}", kind.stage_name(), error.trim()));
    }
    if let Some(selected) = status.strip_prefix("success:") {
        let selected = selected.trim();
        if !selected.is_empty() {
            return Ok(selected);
        }
    }
    Err(format!(
        "{}: unexpected switch status: {status}",
        kind.stage_name()
    ))
}

fn switch_semantic_option(
    config_path: &str,
    provider: Provider,
    target_aliases: &[&str],
    verification_aliases: &[&str],
    kind: SelectionKind,
    verbose: bool,
) -> Result<(), String> {
    let capability_supported = match kind {
        SelectionKind::Model => provider.capabilities().model_selection,
        SelectionKind::Reasoning => provider.capabilities().reasoning,
    };
    if !capability_supported {
        return Err(format!(
            "{} does not support --{} in this ask-bridge version.",
            provider.display_name(),
            kind.display_name()
        ));
    }
    if provider == Provider::Claude {
        return Err("Semantic option selection is not supported for Claude".to_string());
    }
    let primary_target = target_aliases
        .first()
        .ok_or_else(|| format!("No {} target was provided", kind.display_name()))?;
    if verification_aliases.is_empty() {
        return Err(format!(
            "No {} verification aliases were provided",
            kind.display_name()
        ));
    }

    let request = serde_json::json!({
        "provider": provider.to_string(),
        "kind": kind.display_name(),
        "targetAliases": target_aliases,
        "verificationAliases": verification_aliases,
    });
    let request_json = serde_json::to_string(&request)
        .map_err(|error| format!("Failed to serialize selection request: {error}"))?;
    let helper = include_str!("model-selection.cjs");
    let js = format!(
        r#"() => {{
            {helper}
            window.__ask_bridge_selection_status = 'pending';
            (async () => {{
                try {{
                    const result = await globalThis.AskBridgeModelSelection.selectProviderOption({request_json});
                    if (result.ok) {{
                        window.__ask_bridge_selection_status = 'success:' + result.selected;
                    }} else {{
                        const available = result.available && result.available.length
                            ? '; available options: ' + result.available.join(', ')
                            : '';
                        window.__ask_bridge_selection_status = 'error: ' + result.error + available;
                    }}
                }} catch (error) {{
                    window.__ask_bridge_selection_status = 'error: ' + error.message;
                }}
            }})();
            return true;
        }}"#
    );

    if verbose {
        println!(
            "Switching {} {} to '{}'...",
            provider.display_name(),
            kind.display_name(),
            primary_target
        );
    }

    let start_result = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({ "function": js }),
    )?;
    let start_parsed = parse_script_result(&start_result)?;
    if !start_parsed.as_bool().unwrap_or(false) {
        return Err(format!(
            "Failed to initiate {} switch script",
            kind.display_name()
        ));
    }

    let mut wait_cycles = 0;
    let mut status = String::from("pending");
    // ChatGPT may wait up to 5s for the picker and traverse six nested levels
    // twice when post-click verification must reopen the menu.
    while status == "pending" && wait_cycles < 180 {
        thread::sleep(Duration::from_millis(200));
        let check_result = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({
                "function": "() => window.__ask_bridge_selection_status || 'pending'"
            }),
        )?;
        status = parse_script_result(&check_result)?
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("Invalid {} switch status", kind.display_name()))?;
        wait_cycles += 1;
    }

    let selected = interpret_selection_status(kind, &status)?;

    if verbose {
        println!("{} switched successfully ({selected})", kind.display_name());
    }
    thread::sleep(Duration::from_millis(500));

    Ok(())
}

fn claude_model_switch_script(target_json: &str) -> String {
    let template = r#"() => {
        window.__switch_model_status = 'pending';
        (async () => {
            try {
                const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
                const norm = (s) => (s || '').toLowerCase().replace(/[\s.\-_]/g, '');
                const labelOf = (el) => ((el.innerText || el.textContent || '').split('\n')[0] || '').trim();
                const target = norm(__TARGET_MODEL__);
                if (!target) { window.__switch_model_status = 'error: empty target'; return; }
                document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', keyCode: 27, bubbles: true }));
                await sleep(300);
                let trigger = document.querySelector('[data-testid="model-selector-dropdown"]');
                if (!trigger) {
                    trigger = Array.from(document.querySelectorAll('button')).find((button) => {
                        const popup = button.getAttribute('aria-haspopup');
                        if (popup !== 'menu' && popup !== 'listbox') return false;
                        const label = [button.getAttribute('aria-label'), button.textContent].filter(Boolean).join(' ');
                        return /model|claude|opus|sonnet|haiku|fable/i.test(label);
                    });
                }
                if (!trigger) { window.__switch_model_status = 'error: Claude model selector not found'; return; }
                trigger.click();
                await sleep(800);
                const visited = new Set();
                let clicked = false;
                let chosen = '';
                for (let depth = 0; depth < 4 && !clicked; depth++) {
                    const items = Array.from(document.querySelectorAll('[role="menuitem"], [role="option"], [role="menuitemradio"]'));
                    const leaves = items.filter((it) => it.getAttribute('aria-haspopup') !== 'menu');
                    let match = leaves.find((it) => norm(labelOf(it)) === target);
                    if (!match) match = leaves.find((it) => norm(labelOf(it)).startsWith(target));
                    if (match) {
                        match.click();
                        clicked = true;
                        chosen = labelOf(match);
                        break;
                    }
                    const trigs = items.filter((it) => it.getAttribute('aria-haspopup') === 'menu');
                    const trig = trigs.find((it) => !visited.has(norm(it.innerText)));
                    if (!trig) break;
                    visited.add(norm(trig.innerText));
                    trig.dispatchEvent(new MouseEvent('pointerenter', { bubbles: true }));
                    trig.dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
                    trig.click();
                    await sleep(700);
                }
                document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', keyCode: 27, bubbles: true }));
                if (!clicked) {
                    window.__switch_model_status = 'error: model not found in menu';
                    return;
                }
                await sleep(400);
                window.__switch_model_status = 'success:' + chosen;
            } catch (e) {
                window.__switch_model_status = 'error: ' + e.message;
            }
        })();
        return true;
    }"#;
    template.replace("__TARGET_MODEL__", target_json)
}

/// Switch the selected provider to the specified model. ChatGPT and Gemini use
/// exact primary-label matching; Claude retains its existing selector path.
fn switch_model(
    config_path: &str,
    provider: Provider,
    model: &str,
    verbose: bool,
) -> Result<(), String> {
    if model.trim().is_empty() {
        return Err("Empty model name".to_string());
    }
    if !provider.capabilities().model_selection {
        return Err(format!(
            "{} does not support --model in this ask-bridge version.",
            provider.display_name()
        ));
    }
    if provider != Provider::Claude {
        return switch_semantic_option(
            config_path,
            provider,
            &[model.trim()],
            &[model.trim()],
            SelectionKind::Model,
            verbose,
        );
    }
    let target_json = serde_json::to_string(model.trim())
        .map_err(|e| format!("Failed to serialize model name: {}", e))?;

    if verbose {
        println!(
            "Switching {} model to '{}'...",
            provider.display_name(),
            model.trim()
        );
    }

    let js = claude_model_switch_script(&target_json);

    let start_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({ "function": js }),
    )?;
    let start_parsed = parse_script_result(&start_res)?;
    if !start_parsed.as_bool().unwrap_or(false) {
        return Err("Failed to initiate model switch script".to_string());
    }

    let mut wait_cycles = 0;
    let mut status = String::from("pending");
    while status == "pending" && wait_cycles < 60 {
        thread::sleep(Duration::from_millis(200));
        let check_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({ "function": "() => window.__switch_model_status || 'pending'" }),
        )?;
        if let Some(s) = parse_script_result(&check_res)
            .ok()
            .and_then(|p| p.as_str().map(|r| r.to_string()))
        {
            status = s;
        }
        wait_cycles += 1;
    }

    if status.starts_with("error:") {
        return Err(format!("Model switch failed: {}", status));
    }
    if status == "pending" {
        return Err("Timed out waiting for model switch".to_string());
    }

    if verbose {
        println!("Model switched successfully ({})", status);
    }

    // Give the UI a moment to settle after switching models
    thread::sleep(Duration::from_millis(500));

    Ok(())
}

fn switch_reasoning(
    config_path: &str,
    provider: Provider,
    reasoning: ReasoningRequest,
    verbose: bool,
) -> Result<(), String> {
    if !provider.capabilities().reasoning {
        return Err(format!(
            "{} does not support --reasoning in this ask-bridge version.",
            provider.display_name()
        ));
    }
    switch_semantic_option(
        config_path,
        provider,
        reasoning.target_aliases(),
        reasoning.verification_aliases(),
        SelectionKind::Reasoning,
        verbose,
    )
}

fn wait_for_submit_status(config_path: &str) -> Result<String, String> {
    let mut wait_cycles = 0;
    let mut status = String::from("pending");

    // Page-side submission scripts may wait up to 15s for ChatGPT/Gemini to
    // enable the send button, so keep this host-side polling window longer.
    while status == "pending" && wait_cycles < 180 {
        thread::sleep(Duration::from_millis(100));
        let check_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({
                "function": "() => window.__submit_status || 'pending'"
            }),
        )?;
        if let Some(s) = parse_script_result(&check_res)
            .ok()
            .and_then(|p| p.as_str().map(|str_ref| str_ref.to_string()))
        {
            status = s;
        }
        wait_cycles += 1;
    }

    if status.starts_with("error:") {
        return Err(status);
    }

    if status == "pending" {
        return Err("Timed out waiting for send button to activate and submit".to_string());
    }

    Ok(status)
}

fn focus_and_clear_composer(config_path: &str, provider: Provider) -> Result<(), String> {
    let js = r#"() => {
            const composerSelectors = __COMPOSER_SELECTORS__;
            const el = composerSelectors.map((s) => document.querySelector(s)).find(Boolean);
            if (!el) {
                return { ok: false, error: 'composer not found' };
            }

            el.focus();
            try {
                const range = document.createRange();
                range.selectNodeContents(el);
                const sel = window.getSelection();
                sel.removeAllRanges();
                sel.addRange(range);
                document.execCommand('delete');
            } catch (e) {}

            const currentText = typeof el.value !== 'undefined' ? el.value : (el.innerText || el.textContent || '');
            if ((currentText || '').trim().length > 0) {
                if (typeof el.value !== 'undefined') {
                    el.value = '';
                    if (el._valueTracker) {
                        el._valueTracker.setValue('');
                    }
                } else {
                    el.innerHTML = '<p><br></p>';
                }
                el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'deleteContentBackward' }));
                el.dispatchEvent(new Event('change', { bubbles: true }));
            }

            el.focus();
            return { ok: true };
        }"#
    .replace("__COMPOSER_SELECTORS__", provider.composer_selectors_json());

    let res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({ "function": js }),
    )?;
    let parsed = parse_script_result(&res)?;
    if parsed
        .get("ok")
        .and_then(|ok| ok.as_bool())
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err(parsed
            .get("error")
            .and_then(|err| err.as_str())
            .unwrap_or("failed to focus and clear composer")
            .to_string())
    }
}

fn wait_for_chatgpt_agent_menu(config_path: &str) -> Result<(), String> {
    let js = r#"() => {
            const isVisible = (el) => {
                if (!el) return false;
                const style = window.getComputedStyle(el);
                if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
                const rect = el.getBoundingClientRect();
                return rect.width > 0 && rect.height > 0;
            };
            const composer = document.querySelector('#prompt-textarea');
            const composerRect = composer ? composer.getBoundingClientRect() : null;
            const isNearComposer = (el) => {
                if (!composerRect) return true;
                const rect = el.getBoundingClientRect();
                const itemCenterX = (rect.left + rect.right) / 2;
                const composerCenterX = (composerRect.left + composerRect.right) / 2;
                const maxHorizontalDistance = Math.max(500, composerRect.width);
                return Math.abs(itemCenterX - composerCenterX) <= maxHorizontalDistance &&
                    Math.abs(rect.top - composerRect.bottom) <= 500;
            };
            const items = Array.from(document.querySelectorAll(
                '.popover .__menu-item, [class*="popover"] .__menu-item, [role="menuitem"], [role="option"], [cmdk-item]'
            ))
                .filter((el) => isVisible(el) && isNearComposer(el))
                .map((el) => (el.innerText || el.textContent || '').trim())
                .filter(Boolean);

            return { ok: items.length > 0, items: items.slice(0, 5) };
        }"#;

    let mut last_state = String::new();
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(125));
        let res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({ "function": js }),
        )?;
        let parsed = parse_script_result(&res)?;
        if parsed
            .get("ok")
            .and_then(|ok| ok.as_bool())
            .unwrap_or(false)
        {
            return Ok(());
        }
        last_state = parsed.to_string();
    }

    Err(format!(
        "Timed out waiting for ChatGPT agent menu after typing mention ({})",
        last_state
    ))
}

fn wait_for_chatgpt_agent_selection(config_path: &str) -> Result<(), String> {
    let js = r#"() => {
            const composer = document.querySelector('#prompt-textarea');
            if (!composer) {
                return { ok: false, error: 'composer not found' };
            }
            const agentPill = composer.querySelector(
                '[data-id="agent"], [data-system-hint-type="agent"], [data-symbol="ecosystemMention"], [data-inline-selection-pill][contenteditable="false"]'
            );
            return {
                ok: Boolean(agentPill),
                text: (composer.innerText || composer.textContent || '').trim(),
                keyword: agentPill ? (agentPill.getAttribute('data-keyword') || agentPill.textContent || '').trim() : ''
            };
        }"#;

    let mut last_state = String::new();
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(125));
        let res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({ "function": js }),
        )?;
        let parsed = parse_script_result(&res)?;
        if parsed
            .get("ok")
            .and_then(|ok| ok.as_bool())
            .unwrap_or(false)
        {
            return Ok(());
        }
        last_state = parsed.to_string();
    }

    Err(format!(
        "Timed out waiting for ChatGPT agent selection after Tab ({})",
        last_state
    ))
}

fn submit_regular_prompt(
    config_path: &str,
    provider: Provider,
    prompt: &str,
) -> Result<String, String> {
    let prompt_json = serde_json::to_string(prompt)
        .map_err(|e| format!("Failed to serialize prompt text: {}", e))?;
    let set_and_submit_js = r#"() => {
            window.__submit_status = 'pending';
            (async () => {
                try {
                    const composerSelectors = __COMPOSER_SELECTORS__;
                    const sendSelectors = __SEND_SELECTORS__;
                    const el = composerSelectors.map((s) => document.querySelector(s)).find(Boolean);
                    if (!el) {
                        window.__submit_status = 'error: composer not found';
                        return;
                    }
                    el.focus();
                    
                    const value = __PROMPT__;
                    el.focus();
                    
                    try {
                        const range = document.createRange();
                        range.selectNodeContents(el);
                        const sel = window.getSelection();
                        sel.removeAllRanges();
                        sel.addRange(range);
                    } catch (e) {}
                    
                    let pasted = false;
                    try {
                        const dataTransfer = new DataTransfer();
                        dataTransfer.setData('text/plain', value);
                        const event = new ClipboardEvent('paste', {
                            bubbles: true,
                            cancelable: true
                        });
                        Object.defineProperty(event, 'clipboardData', {
                            value: dataTransfer,
                            writable: false,
                            configurable: true
                        });
                        el.dispatchEvent(event);
                        
                        const currentText = typeof el.value !== 'undefined' ? el.value : el.textContent;
                        if (currentText && currentText.trim().length > 0) {
                            pasted = true;
                        }
                    } catch (e) {}
                    
                    if (!pasted) {
                        const success = document.execCommand('insertText', false, value);
                        if (!success) {
                            if (typeof el.value !== 'undefined') {
                                el.value = value;
                                if (el._valueTracker) {
                                    el._valueTracker.setValue('');
                                }
                            } else {
                                el.innerText = value;
                            }
                            el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: value }));
                            el.dispatchEvent(new Event('change', { bubbles: true }));
                        }
                    }
                    
                    const isVisible = (el) => {
                        if (!el || el.disabled || el.getAttribute('aria-disabled') === 'true') return false;
                        const style = window.getComputedStyle(el);
                        if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
                        const rect = el.getBoundingClientRect();
                        return rect.width > 0 && rect.height > 0;
                    };
                    const findAndClickSendButton = () => {
                        let btn = null;
                        for (const s of sendSelectors) {
                            btn = document.querySelector(s);
                            if (isVisible(btn)) break;
                        }
                        
                        if (btn && !btn.disabled && btn.getAttribute('aria-disabled') !== 'true') {
                            btn.click();
                            return { ok: true, clicked: true, buttonLabel: btn.getAttribute('aria-label') };
                        }
                        return null;
                    };
                    
                    let result = findAndClickSendButton();
                    if (result) {
                        window.__submit_status = 'success:' + JSON.stringify(result);
                        return;
                    }

                    for (let i = 0; i < 150; i++) {
                        await new Promise(r => setTimeout(r, 100));
                        result = findAndClickSendButton();
                        if (result) {
                            window.__submit_status = 'success:' + JSON.stringify(result);
                            return;
                        }
                    }
                    
                    window.__submit_status = 'error: Send button did not become active/enabled';
                } catch (e) {
                    window.__submit_status = 'error: ' + e.message;
                }
            })();
            return true;
        }"#
    .replace("__COMPOSER_SELECTORS__", provider.composer_selectors_json())
    .replace("__SEND_SELECTORS__", provider.send_button_selectors_json())
    .replace("__PROMPT__", &prompt_json);

    let start_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": set_and_submit_js
        }),
    )?;

    let start_parsed = parse_script_result(&start_res)?;
    if !start_parsed.as_bool().unwrap_or(false) {
        return Err("Failed to initiate text entry and submission script".to_string());
    }

    wait_for_submit_status(config_path)
}

fn submit_m365_prompt(config_path: &str, prompt: &str) -> Result<String, String> {
    let snapshot = take_snapshot_text(config_path)?;
    let composer_uid = find_m365_composer_uid(&snapshot)
        .ok_or_else(|| "M365 composer textbox not found in page snapshot".to_string())?;
    call_mcp_tool(
        config_path,
        "fill",
        serde_json::json!({
            "uid": composer_uid,
            "value": "",
            "includeSnapshot": false
        }),
    )
    .map_err(|error| format!("M365 composer clear failed: {error}"))?;

    let focus_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": r#"() => {
                const composer = document.querySelector('#m365-chat-editor-target-element')
                    || document.querySelector('[role="textbox"][contenteditable="true"][aria-label*="Copilot"]');
                if (!composer) return { ok: false, error: 'composer not found' };
                const text = (composer.innerText || composer.textContent || '')
                    .replace(/[\u200b-\u200d\u2060\ufeff]/g, '')
                    .trim();
                if (text.length > 0) {
                    return { ok: false, error: 'composer could not be cleared before typing' };
                }
                composer.focus();
                return { ok: true };
            }"#
        }),
    )?;
    let focus_parsed = parse_script_result(&focus_res)?;
    if !focus_parsed
        .get("ok")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return Err(focus_parsed
            .get("error")
            .and_then(|value| value.as_str())
            .unwrap_or("failed to focus M365 composer")
            .to_string());
    }
    call_mcp_tool(
        config_path,
        "type_text",
        serde_json::json!({
            "text": prompt
        }),
    )
    .map_err(|error| format!("M365 composer text entry failed: {error}"))?;

    let prompt_json = serde_json::to_string(prompt)
        .map_err(|error| format!("Failed to serialize prompt text: {error}"))?;
    let helper = include_str!("m365-automation.cjs");
    let submit_js = format!(
        r#"() => {{
            window.__submit_status = 'pending';
            (async () => {{
                try {{
                    {helper}
                    const prompt = {prompt_json};
                    const composerSelectors = {};
                    const sendSelectors = {};
                    const stopSelectors = {};
                    const sleep = (delay) => new Promise((resolve) => setTimeout(resolve, delay));
                    const isVisible = (element) => {{
                        if (!element || element.disabled || element.getAttribute('aria-disabled') === 'true') {{
                            return false;
                        }}
                        const style = window.getComputedStyle(element);
                        if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') {{
                            return false;
                        }}
                        const rect = element.getBoundingClientRect();
                        return rect.width > 0 && rect.height > 0;
                    }};
                    const findComposer = () => composerSelectors
                        .map((selector) => document.querySelector(selector))
                        .find(Boolean);
                    const readComposerText = () => {{
                        const composer = findComposer();
                        return composer
                            ? (composer.innerText || composer.textContent || '')
                            : '';
                    }};
                    const findVisibleButton = (selectors) => selectors
                        .map((selector) => document.querySelector(selector))
                        .find(isVisible);

                    let readySamples = 0;
                    let stableMismatch = '';
                    let stableMismatchSamples = 0;
                    for (let attempt = 0; attempt < 100; attempt += 1) {{
                        const inspection = globalThis.AskBridgeM365Automation
                            .classifyComposerPrompt(prompt, readComposerText());
                        if (inspection.status === 'ready') {{
                            readySamples += 1;
                            stableMismatch = '';
                            stableMismatchSamples = 0;
                            if (readySamples >= 2) break;
                        }} else {{
                            readySamples = 0;
                            if (inspection.status === 'duplicate') {{
                                window.__submit_status =
                                    'error: M365 composer contains a duplicated prompt; submission was cancelled';
                                return;
                            }}
                            if (inspection.status === 'mismatch') {{
                                const signature = JSON.stringify(inspection);
                                if (signature === stableMismatch) {{
                                    stableMismatchSamples += 1;
                                }} else {{
                                    stableMismatch = signature;
                                    stableMismatchSamples = 1;
                                }}
                                if (stableMismatchSamples >= 5) {{
                                    window.__submit_status =
                                        'error: M365 composer text did not match the requested prompt';
                                    return;
                                }}
                            }}
                        }}
                        await sleep(100);
                    }}
                    if (readySamples < 2) {{
                        window.__submit_status =
                            'error: M365 composer did not stabilize with the requested prompt';
                        return;
                    }}

                    let sendButton = null;
                    for (let attempt = 0; attempt < 100; attempt += 1) {{
                        sendButton = findVisibleButton(sendSelectors);
                        if (sendButton) break;
                        await sleep(100);
                    }}
                    if (!sendButton) {{
                        window.__submit_status =
                            'error: M365 Send button did not become active/enabled';
                        return;
                    }}

                    sendButton.click();
                    for (let attempt = 0; attempt < 100; attempt += 1) {{
                        await sleep(100);
                        const composer = findComposer();
                        const composerText = composer
                            ? globalThis.AskBridgeM365Automation.normalizePromptText(
                                composer.innerText || composer.textContent || ''
                            )
                            : '';
                        const stopButton = findVisibleButton(stopSelectors);
                        if (!composer || composerText.length === 0 || stopButton) {{
                            window.__submit_status = 'success:{{"clicked":true,"accepted":true}}';
                            return;
                        }}
                    }}

                    window.__submit_status =
                        'error: M365 did not accept the Send button click';
                }} catch (error) {{
                    window.__submit_status = 'error: ' + error.message;
                }}
            }})();
            return true;
        }}"#,
        Provider::M365Copilot.composer_selectors_json(),
        Provider::M365Copilot.send_button_selectors_json(),
        Provider::M365Copilot.stop_button_selectors_json(),
    );

    let start_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": submit_js
        }),
    )?;
    let start_parsed = parse_script_result(&start_res)?;
    if !start_parsed.as_bool().unwrap_or(false) {
        return Err("Failed to initiate M365 prompt submission script".to_string());
    }

    wait_for_submit_status(config_path)
}

fn submit_chatgpt_agent_prompt(
    config_path: &str,
    parts: &ChatGptAgentPrompt<'_>,
    verbose: bool,
) -> Result<String, String> {
    if verbose {
        println!(
            "Selecting ChatGPT agent '{}' before submitting prompt...",
            parts.agent_mention
        );
    }

    focus_and_clear_composer(config_path, Provider::ChatGpt)?;
    call_mcp_tool(
        config_path,
        "type_text",
        serde_json::json!({
            "text": parts.agent_mention
        }),
    )?;
    wait_for_chatgpt_agent_menu(config_path)?;
    call_mcp_tool(
        config_path,
        "press_key",
        serde_json::json!({
            "key": "Tab",
            "includeSnapshot": false
        }),
    )?;
    wait_for_chatgpt_agent_selection(config_path)?;

    let body_json = serde_json::to_string(parts.body)
        .map_err(|e| format!("Failed to serialize prompt body: {}", e))?;
    let paste_and_submit_js = r#"() => {
            window.__submit_status = 'pending';
            (async () => {
                try {
                    const sendSelectors = __SEND_SELECTORS__;
                    const el = document.querySelector('#prompt-textarea');
                    if (!el) {
                        window.__submit_status = 'error: composer not found';
                        return;
                    }
                    const agentPill = el.querySelector(
                        '[data-id="agent"], [data-system-hint-type="agent"], [data-symbol="ecosystemMention"], [data-inline-selection-pill][contenteditable="false"]'
                    );
                    if (!agentPill) {
                        window.__submit_status = 'error: ChatGPT agent was not selected into the composer';
                        return;
                    }

                    const body = __BODY__;
                    const currentText = el.textContent || '';
                    const value = currentText && !/\s$/.test(currentText) ? ' ' + body : body;
                    el.focus();

                    try {
                        const range = document.createRange();
                        range.selectNodeContents(el);
                        range.collapse(false);
                        const sel = window.getSelection();
                        sel.removeAllRanges();
                        sel.addRange(range);
                    } catch (e) {}

                    let pasted = false;
                    try {
                        const dataTransfer = new DataTransfer();
                        dataTransfer.setData('text/plain', value);
                        const event = new ClipboardEvent('paste', {
                            bubbles: true,
                            cancelable: true
                        });
                        Object.defineProperty(event, 'clipboardData', {
                            value: dataTransfer,
                            writable: false,
                            configurable: true
                        });
                        el.dispatchEvent(event);
                        const afterPasteText = el.innerText || el.textContent || '';
                        pasted = afterPasteText.includes(body);
                    } catch (e) {}

                    if (!pasted) {
                        const success = document.execCommand('insertText', false, value);
                        if (!success) {
                            el.appendChild(document.createTextNode(value));
                            el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: value }));
                            el.dispatchEvent(new Event('change', { bubbles: true }));
                        }
                    }

                    const afterText = el.innerText || el.textContent || '';
                    if (!afterText.includes(body)) {
                        window.__submit_status = 'error: prompt body was not pasted after ChatGPT agent selection';
                        return;
                    }

                    const isVisible = (el) => {
                        if (!el || el.disabled || el.getAttribute('aria-disabled') === 'true') return false;
                        const style = window.getComputedStyle(el);
                        if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
                        const rect = el.getBoundingClientRect();
                        return rect.width > 0 && rect.height > 0;
                    };
                    const findAndClickSendButton = () => {
                        let btn = null;
                        for (const s of sendSelectors) {
                            btn = document.querySelector(s);
                            if (isVisible(btn)) break;
                        }
                        if (btn && !btn.disabled && btn.getAttribute('aria-disabled') !== 'true') {
                            btn.click();
                            return { ok: true, clicked: true, buttonLabel: btn.getAttribute('aria-label') };
                        }
                        return null;
                    };

                    let result = findAndClickSendButton();
                    if (result) {
                        window.__submit_status = 'success:' + JSON.stringify(result);
                        return;
                    }

                    for (let i = 0; i < 150; i++) {
                        await new Promise(r => setTimeout(r, 100));
                        result = findAndClickSendButton();
                        if (result) {
                            window.__submit_status = 'success:' + JSON.stringify(result);
                            return;
                        }
                    }

                    window.__submit_status = 'error: Send button did not become active/enabled';
                } catch (e) {
                    window.__submit_status = 'error: ' + e.message;
                }
            })();
            return true;
        }"#
    .replace(
        "__SEND_SELECTORS__",
        Provider::ChatGpt.send_button_selectors_json(),
    )
    .replace("__BODY__", &body_json);

    let start_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": paste_and_submit_js
        }),
    )?;
    let start_parsed = parse_script_result(&start_res)?;
    if !start_parsed.as_bool().unwrap_or(false) {
        return Err("Failed to initiate ChatGPT agent prompt submission script".to_string());
    }

    wait_for_submit_status(config_path)
}

fn submit_prompt_to_provider(
    config_path: &str,
    provider: Provider,
    prompt: &str,
    verbose: bool,
) -> Result<String, String> {
    if provider == Provider::ChatGpt
        && let Some(parts) = parse_chatgpt_agent_prompt(prompt)
    {
        return submit_chatgpt_agent_prompt(config_path, &parts, verbose);
    }
    if provider == Provider::M365Copilot {
        return submit_m365_prompt(config_path, prompt);
    }

    submit_regular_prompt(config_path, provider, prompt)
}

fn ensure_provider_tab(
    config_path: &str,
    provider: Provider,
    force_new: bool,
    headless: bool,
    verbose: bool,
) -> Result<(), String> {
    if verbose {
        println!("Checking open Chrome tabs...");
    }
    let list_res = call_mcp_tool(config_path, "list_pages", serde_json::json!({}))?;

    let text = list_res
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|obj| obj.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| format!("Invalid list_pages response structure: {:?}", list_res))?;

    let pages = parse_pages(text);

    let mut new_session_page_id = None;

    if force_new {
        if verbose {
            println!("Opening a brand new {} session...", provider.display_name());
        }
        call_mcp_tool(
            config_path,
            "new_page",
            serde_json::json!({
                "url": provider.home_url()
            }),
        )?;

        let refreshed_pages_res = call_mcp_tool(config_path, "list_pages", serde_json::json!({}))?;
        let refreshed_text = refreshed_pages_res
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|obj| obj.get("text"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| {
                format!(
                    "Invalid refreshed list_pages response structure: {:?}",
                    refreshed_pages_res
                )
            })?;
        let refreshed_pages = parse_pages(refreshed_text);
        let new_page_id = unique_new_page_id(&pages, &refreshed_pages)?;

        if verbose {
            println!(
                "Selecting new {} tab (ID: {}) while preserving existing tabs...",
                provider.display_name(),
                new_page_id
            );
        }
        call_mcp_tool(
            config_path,
            "select_page",
            serde_json::json!({
                "pageId": new_page_id,
                "bringToFront": !headless
            }),
        )?;
        new_session_page_id = Some(new_page_id);
    } else {
        let provider_pages: Vec<&Page> = pages
            .iter()
            .filter(|page| provider.owns_url(&page.url))
            .collect();

        let provider_page_id = if provider_pages.len() > 1 {
            let mut page_states = Vec::with_capacity(provider_pages.len());
            for page in &provider_pages {
                call_mcp_tool(
                    config_path,
                    "select_page",
                    serde_json::json!({
                        "pageId": page.id,
                        "bringToFront": false
                    }),
                )?;
                let login_state = check_login_status(config_path, provider, verbose)
                    .unwrap_or(LoginState::Unknown);
                page_states.push(PageLoginState {
                    id: page.id,
                    selected: page.selected,
                    login_state,
                });
            }
            preferred_provider_page_id(&page_states)
        } else {
            provider_pages.first().map(|page| page.id)
        };

        match provider_page_id {
            Some(page_id) => {
                let page = provider_pages
                    .iter()
                    .find(|page| page.id == page_id)
                    .ok_or_else(|| "Selected provider page disappeared".to_string())?;
                if verbose {
                    println!(
                        "Found {} tab (ID: {}, selected: {}). Selecting/focusing...",
                        provider.display_name(),
                        page.id,
                        page.selected
                    );
                }
                call_mcp_tool(
                    config_path,
                    "select_page",
                    serde_json::json!({
                        "pageId": page.id,
                        "bringToFront": !headless
                    }),
                )?;
            }
            None => {
                // No provider tab. If there is only one blank tab, navigate it. Otherwise open a new page.
                if pages.len() == 1
                    && (pages[0].url == "about:blank"
                        || pages[0].url.contains("new-tab-page")
                        || pages[0].url.contains("chrome://welcome"))
                {
                    if verbose {
                        println!(
                            "Navigating existing blank tab to {}...",
                            provider.display_name()
                        );
                    }
                    call_mcp_tool(
                        config_path,
                        "navigate_page",
                        serde_json::json!({
                            "url": provider.home_url()
                        }),
                    )?;
                } else {
                    if verbose {
                        println!("Opening a new tab for {}...", provider.display_name());
                    }
                    call_mcp_tool(
                        config_path,
                        "new_page",
                        serde_json::json!({
                            "url": provider.home_url()
                        }),
                    )?;
                }
            }
        }
    }

    // Wait for the provider composer to be present.
    if verbose {
        println!("Waiting for {} to load...", provider.display_name());
    }
    for attempt in 0..90 {
        if attempt > 0 && attempt % 10 == 0 {
            if let Some(page_id) = new_session_page_id {
                let current_pages_res =
                    call_mcp_tool(config_path, "list_pages", serde_json::json!({}))?;
                let current_pages_text = current_pages_res
                    .get("content")
                    .and_then(|content| content.as_array())
                    .and_then(|items| items.first())
                    .and_then(|item| item.get("text"))
                    .and_then(|text| text.as_str())
                    .ok_or_else(|| {
                        format!(
                            "Invalid list_pages response while verifying new tab: {:?}",
                            current_pages_res
                        )
                    })?;
                let page = parse_pages(current_pages_text)
                    .into_iter()
                    .find(|page| page.id == page_id)
                    .ok_or_else(|| {
                        format!(
                            "New {} tab (ID: {}) disappeared; refusing to reuse an existing tab",
                            provider.display_name(),
                            page_id
                        )
                    })?;

                call_mcp_tool(
                    config_path,
                    "select_page",
                    serde_json::json!({
                        "pageId": page.id,
                        "bringToFront": !headless
                    }),
                )?;
                continue;
            }

            let page_opt = call_mcp_tool(config_path, "list_pages", serde_json::json!({}))
                .ok()
                .and_then(|response| {
                    response
                        .get("content")
                        .and_then(|c| c.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|obj| obj.get("text"))
                        .and_then(|t| t.as_str())
                        .map(|t| t.to_string())
                })
                .and_then(|text| {
                    parse_pages(&text)
                        .into_iter()
                        .find(|page| provider.owns_url(&page.url))
                });
            if let Some(page) = page_opt {
                let _ = call_mcp_tool(
                    config_path,
                    "select_page",
                    serde_json::json!({
                        "pageId": page.id,
                        "bringToFront": !headless
                    }),
                );
            }
        }

        let ready_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({
                "function": provider.ready_check_js()
            }),
        );
        let ready_res = match ready_res {
            Ok(res) => res,
            Err(e) => {
                if verbose {
                    eprintln!(
                        "Warning: Failed to check {} readiness: {}",
                        provider.display_name(),
                        e
                    );
                }
                thread::sleep(Duration::from_millis(500));
                continue;
            }
        };
        if let Ok(parsed) = parse_script_result(&ready_res) {
            let is_ready = parsed.as_bool().unwrap_or(false);
            if is_ready {
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(500));
    }

    Err(format!(
        "Timeout waiting for {} page to load",
        provider.display_name()
    ))
}

fn check_login_status(
    config_path: &str,
    provider: Provider,
    verbose: bool,
) -> Result<LoginState, String> {
    let res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": provider.login_signals_js()
        }),
    )?;

    let parsed = parse_script_result(&res)?;
    let signals: LoginSignals = serde_json::from_value(parsed)
        .map_err(|e| format!("Failed to parse login signals: {}", e))?;
    if verbose {
        println!(
            "{} login signals: account={}, auth_control={}, auth_path={}, composer={}, stable={}",
            provider.display_name(),
            signals.account,
            signals.auth_control,
            signals.auth_path,
            signals.composer,
            signals.stable
        );
    }
    Ok(signals.state(provider))
}

fn wait_for_login_completion(
    config_path: &str,
    provider: Provider,
    timeout_seconds: u64,
    verbose: bool,
) -> (LoginState, bool) {
    let timeout = Duration::from_secs(timeout_seconds.max(1));
    let start = Instant::now();
    let display_name = provider.display_name();

    if verbose {
        println!(
            "Waiting for {} login status every second (timeout: {} seconds)...",
            display_name,
            timeout_seconds.max(1)
        );
    } else {
        println!("Waiting for login completion (checking every second)...");
    }

    loop {
        let state = match check_login_status(config_path, provider, verbose) {
            Ok(state) => state,
            Err(e) => {
                if verbose {
                    println!(
                        "Warning: Failed to check {} login status: {}",
                        display_name, e
                    );
                }
                LoginState::Unknown
            }
        };

        if state == LoginState::LoggedIn {
            return (LoginState::LoggedIn, false);
        }

        if start.elapsed() >= timeout {
            return (state, true);
        }

        thread::sleep(Duration::from_secs(1));
    }
}

fn print_chrome_diagnostics(profile_path: &str) {
    let snapshot = inspect_chrome_debug_port(profile_path);
    let recorded_pid = read_chrome_pid().unwrap_or_else(|| "unknown".to_string());

    println!("Chrome diagnostics:");
    println!("  profile: {}", profile_path);
    println!("  recorded PID: {}", recorded_pid);
    println!("  listener PIDs: {:?}", snapshot.listener_pids);
    println!("  ask-bridge owner PIDs: {:?}", snapshot.ask_pids);
    println!(
        "  CDP browser identity recorded: {}",
        snapshot
            .record
            .and_then(|record| record.browser_id)
            .is_some()
    );
}

/// How long to wait for a non-tty stdin to produce its first byte (or EOF)
/// when a prompt argument was already provided. Agent harnesses (Claude Code,
/// Codex) run commands with a pipe they may never close; blocking on EOF hung
/// whole runs (2026-07-11).
const STDIN_PIPE_GRACE: Duration = Duration::from_secs(2);

enum StdinProbe {
    Data,
    Eof,
}

/// Read stdin on a helper thread, signalling the first byte (or EOF) on one
/// channel and the full content on another, so the caller can bound how long
/// it waits for a pipe that might never deliver anything.
fn spawn_stdin_reader() -> (
    std::sync::mpsc::Receiver<StdinProbe>,
    std::sync::mpsc::Receiver<std::io::Result<String>>,
) {
    let (probe_tx, probe_rx) = std::sync::mpsc::channel();
    let (data_tx, data_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut first = [0u8; 1];
        match stdin.read(&mut first) {
            Ok(0) => {
                let _ = probe_tx.send(StdinProbe::Eof);
                let _ = data_tx.send(Ok(String::new()));
            }
            Ok(_) => {
                let _ = probe_tx.send(StdinProbe::Data);
                let mut bytes = vec![first[0]];
                let result = stdin.read_to_end(&mut bytes).and_then(|_| {
                    String::from_utf8(bytes)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
                });
                let _ = data_tx.send(result);
            }
            Err(e) => {
                let _ = probe_tx.send(StdinProbe::Eof);
                let _ = data_tx.send(Err(e));
            }
        }
    });
    (probe_rx, data_rx)
}

/// With a prompt argument in hand piped stdin is an optional supplement: wait
/// up to `grace` for the pipe's first byte, then read a live pipe to EOF as
/// before; a silent pipe (agent harness holding it open) is treated as "no
/// piped input". Without a prompt argument stdin IS the prompt, so wait
/// unbounded exactly like upstream.
fn recv_piped_stdin(
    probe_rx: &std::sync::mpsc::Receiver<StdinProbe>,
    data_rx: &std::sync::mpsc::Receiver<std::io::Result<String>>,
    grace: Duration,
    has_prompt_argument: bool,
) -> std::io::Result<String> {
    if !has_prompt_argument {
        // stdin IS the prompt: wait unbounded like upstream, but after the
        // grace window tell the user what we are blocked on (an agent harness
        // holding the pipe open would otherwise hang here with no diagnostic).
        return match data_rx.recv_timeout(grace) {
            Ok(result) => result,
            Err(_) => {
                eprintln!(
                    "Waiting for a prompt on stdin (pipe is open; close it or pass a prompt argument)..."
                );
                data_rx.recv().unwrap_or(Ok(String::new()))
            }
        };
    }
    match probe_rx.recv_timeout(grace) {
        Ok(_) => data_rx.recv().unwrap_or(Ok(String::new())),
        Err(_) => {
            eprintln!(
                "No piped stdin data within {}s; continuing with the prompt argument only.",
                grace.as_secs()
            );
            Ok(String::new())
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut cli = Cli::parse();
    if cli.command.is_none() {
        let is_stdin_terminal = io::stdin().is_terminal();
        if is_stdin_terminal && cli.prompt.as_deref() == Some("update") {
            cli.command = Some(Commands::Update);
        }
    }

    let command_verbose = match &cli.command {
        Some(Commands::Get { verbose, .. }) => cli.verbose || *verbose,
        _ => cli.verbose,
    };

    FORWARD_MCP_STDERR.store(command_verbose, std::sync::atomic::Ordering::Relaxed);

    if matches!(cli.command, Some(Commands::Config)) {
        if let Err(e) = run_config_command(cli.provider) {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }

        return Ok(());
    }
    if matches!(cli.command, Some(Commands::Update)) {
        if let Err(e) = run_update_command() {
            eprintln!("Update failed: {}", e);
            std::process::exit(1);
        }
        return Ok(());
    }

    let mut provider = match resolve_provider(cli.provider) {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = validate_provider_feature_support(provider, &cli) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }

    let session_target = match session_input(&cli) {
        Some(session) => match resolve_session_target(provider, cli.provider.is_some(), session) {
            Ok(target) => {
                provider = target.0;
                Some(target)
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        },
        None => None,
    };

    if let Err(e) = validate_provider_feature_support(provider, &cli) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }

    if let Err(e) = validate_attachment_inputs(provider, &cli.images, &cli.files) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }

    let selection_plan =
        match resolve_selection_plan(provider, cli.model.as_deref(), cli.reasoning.as_deref()) {
            Ok(plan) => plan,
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        };
    if selection_plan.used_legacy_model {
        eprintln!(
            "Warning: reasoning-like --model values are deprecated; use --reasoning instead."
        );
    }

    if !command_verbose {
        // SAFETY: Called before spawning other threads and before loading MCP config.
        unsafe {
            std::env::remove_var("MCP_DEBUG");
        }
    }
    if std::env::var("MCP_TIMEOUT").is_err() {
        // SAFETY: Called before spawning other threads and before loading MCP config.
        unsafe {
            std::env::set_var("MCP_TIMEOUT", "20");
        }
    }

    let is_terminal = io::stdout().is_terminal();
    let use_glow = is_terminal && is_glow_available();

    let is_headless = match &cli.command {
        Some(Commands::Login) => false, // Force headful only for login command so user can see it to log in
        Some(Commands::Get { .. }) => false, // Default get to headful for debugging by default
        _ => cli.headless, // Respect --headless (defaults to true) for all other commands (including Open)
    };

    if matches!(cli.command, Some(Commands::Close)) {
        let profile_path = match chrome_profile_path() {
            Ok(path) => path,
            Err(e) => {
                eprintln!("Error locating Chrome profile: {}", e);
                std::process::exit(1);
            }
        };

        match close_ask_chrome_on_debug_port(&profile_path) {
            Ok(true) => println!("Closed ask-bridge Chrome browser instance."),
            Ok(false) => println!("No ask-bridge Chrome browser instance is running."),
            Err(e) => {
                eprintln!("Error closing ask-bridge Chrome browser instance: {}", e);
                std::process::exit(1);
            }
        }

        return Ok(());
    }

    if let Err(e) = check_node_runtime() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }

    let config_path = match write_mcp_config(!command_verbose, is_headless) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = start_chrome_if_needed(is_headless, command_verbose) {
        eprintln!("Error starting Chrome: {}", e);
        std::process::exit(1);
    }

    if let Some(command) = cli.command {
        match command {
            Commands::Open { url } => {
                if let Some(url) = url {
                    let page_provider = Provider::from_url(&url).unwrap_or(provider);
                    if let Err(e) = open_url_tab(
                        &config_path,
                        page_provider,
                        &url,
                        is_headless,
                        command_verbose,
                    ) {
                        eprintln!("Error opening URL: {}", e);
                        std::process::exit(1);
                    }

                    match copy_latest_markdown_for_request(
                        &config_path,
                        page_provider,
                        cli.image_output.as_deref(),
                    ) {
                        Ok(markdown) => {
                            if let Some(ref output_path) = cli.output {
                                let _ = std::fs::write(output_path, &markdown).map_err(|e| {
                                    eprintln!("Error writing output file: {}", e);
                                    std::process::exit(1);
                                });
                            }
                            if let Err(e) = render_markdown(&markdown, use_glow) {
                                eprintln!("Error rendering Markdown: {}", e);
                                std::process::exit(1);
                            }
                            if page_provider.capabilities().image_download
                                && let Err(e) = download_images_from_latest_message(
                                    &config_path,
                                    page_provider,
                                    cli.image_output.as_deref(),
                                    command_verbose,
                                )
                            {
                                eprintln!("Error downloading images: {}", e);
                                if page_provider == Provider::M365Copilot
                                    && cli.image_output.is_some()
                                {
                                    std::process::exit(1);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Error copying latest response Markdown: {}", e);
                            std::process::exit(1);
                        }
                    }
                } else {
                    if let Err(e) = ensure_provider_tab(
                        &config_path,
                        provider,
                        false,
                        is_headless,
                        command_verbose,
                    ) {
                        eprintln!("Error ensuring {} tab: {}", provider.display_name(), e);
                        std::process::exit(1);
                    }
                    println!("Successfully opened {}!", provider.display_name());
                }
                return Ok(());
            }
            Commands::Get { url, .. } => {
                let mut page_provider = provider;
                if let Some(url) = url {
                    page_provider = Provider::from_url(&url).unwrap_or(provider);
                    if let Err(e) = open_url_tab(
                        &config_path,
                        page_provider,
                        &url,
                        is_headless,
                        command_verbose,
                    ) {
                        eprintln!("Error opening URL: {}", e);
                        std::process::exit(1);
                    }
                } else {
                    if let Err(e) = ensure_provider_tab(
                        &config_path,
                        provider,
                        false,
                        is_headless,
                        command_verbose,
                    ) {
                        eprintln!("Error ensuring {} tab: {}", provider.display_name(), e);
                        std::process::exit(1);
                    }
                }

                match copy_latest_markdown_for_request(
                    &config_path,
                    page_provider,
                    cli.image_output.as_deref(),
                ) {
                    Ok(markdown) => {
                        if let Some(ref output_path) = cli.output {
                            let _ = std::fs::write(output_path, &markdown).map_err(|e| {
                                eprintln!("Error writing output file: {}", e);
                                std::process::exit(1);
                            });
                        }
                        if let Err(e) = render_markdown(&markdown, use_glow) {
                            eprintln!("Error rendering Markdown: {}", e);
                            std::process::exit(1);
                        }
                        if page_provider.capabilities().image_download
                            && let Err(e) = download_images_from_latest_message(
                                &config_path,
                                page_provider,
                                cli.image_output.as_deref(),
                                command_verbose,
                            )
                        {
                            eprintln!("Error downloading images: {}", e);
                            if page_provider == Provider::M365Copilot && cli.image_output.is_some()
                            {
                                std::process::exit(1);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error copying latest response Markdown: {}", e);
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            Commands::Login => {
                if let Err(e) =
                    ensure_provider_tab(&config_path, provider, false, is_headless, command_verbose)
                {
                    eprintln!("Error ensuring {} tab: {}", provider.display_name(), e);
                    std::process::exit(1);
                }
                println!("\n========================================================");
                println!("Please complete the login manually in the Chrome window.");
                println!("The tool will automatically detect when login is complete every second.");
                println!("========================================================\n");

                let (login_state, timed_out) =
                    wait_for_login_completion(&config_path, provider, cli.timeout, command_verbose);

                match (login_state, timed_out) {
                    (LoginState::LoggedIn, _) => println!(
                        "Success: Logged in successfully! You can now use the `ask-bridge` command."
                    ),
                    (LoginState::LoggedOut, true) => println!(
                        "Warning: Login timeout reached ({} seconds). Login still appears incomplete.",
                        cli.timeout
                    ),
                    (LoginState::Unknown, true) => println!(
                        "Warning: Timeout reached ({} seconds). Login status is still unknown; please verify manually.",
                        cli.timeout
                    ),
                    (LoginState::LoggedOut, false) | (LoginState::Unknown, false) => println!(
                        "Warning: Login status changed while waiting. Please verify the result and rerun if needed."
                    ),
                }
                if command_verbose {
                    match chrome_profile_path() {
                        Ok(profile_path) => print_chrome_diagnostics(&profile_path),
                        Err(e) => eprintln!("Warning: Failed to locate Chrome profile: {}", e),
                    }
                }
                return Ok(());
            }
            Commands::Close => unreachable!("close command is handled before Chrome startup"),
            Commands::Config => unreachable!("config command is handled before Chrome startup"),
            Commands::Update => unreachable!("update command is handled before Chrome startup"),
            Commands::Dump => {
                let list_res = call_mcp_tool(&config_path, "list_pages", serde_json::json!({}))?;
                println!("All pages: {:?}", list_res);
                if let Err(e) =
                    ensure_provider_tab(&config_path, provider, false, is_headless, command_verbose)
                {
                    eprintln!("Error ensuring {} tab: {}", provider.display_name(), e);
                    std::process::exit(1);
                }
                let url_res = call_mcp_tool(
                    &config_path,
                    "evaluate_script",
                    serde_json::json!({
                        "function": "() => window.location.href"
                    }),
                )?;
                println!("Current page URL: {:?}", parse_script_result(&url_res));
                let res = call_mcp_tool(
                    &config_path,
                    "evaluate_script",
                    serde_json::json!({
                        "function": "() => document.body.innerHTML"
                    }),
                )?;
                let html = parse_script_result(&res)?
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                std::fs::create_dir_all("target").unwrap();
                std::fs::write("target/dump.html", html)?;
                println!("Dumped HTML to target/dump.html");
                return Ok(());
            }
            Commands::Screenshot => {
                if let Err(e) =
                    ensure_provider_tab(&config_path, provider, false, is_headless, command_verbose)
                {
                    eprintln!("Error ensuring {} tab: {}", provider.display_name(), e);
                    std::process::exit(1);
                }
                let res = call_mcp_tool(&config_path, "take_screenshot", serde_json::json!({}))?;

                let mut saved = false;
                if let Some(arr) = res.get("content").and_then(|c| c.as_array()) {
                    for item in arr {
                        if let Some(data) = item
                            .get("type")
                            .filter(|t| t.as_str() == Some("image"))
                            .and_then(|_| item.get("data"))
                            .and_then(|d| d.as_str())
                        {
                            use base64::{Engine as _, engine::general_purpose::STANDARD};
                            match STANDARD.decode(data.trim()) {
                                Ok(bytes) => {
                                    std::fs::create_dir_all("target").unwrap();
                                    std::fs::write("target/screenshot.png", bytes)?;
                                    println!("Saved screenshot to target/screenshot.png");
                                    saved = true;
                                    break;
                                }
                                Err(e) => {
                                    eprintln!("Failed to decode base64 image data: {}", e);
                                }
                            }
                        }
                    }
                }
                if !saved {
                    eprintln!(
                        "Could not find any image item in the tool response content. Full response: {:?}",
                        res
                    );
                }
                return Ok(());
            }
        }
    }

    // Read prompt from arguments and optionally append piped stdin content.
    let mut stdin_prompt = String::new();

    // Check if stdin is a pipe (not a tty)
    if !std::io::stdin().is_terminal() {
        let (probe_rx, data_rx) = spawn_stdin_reader();
        stdin_prompt =
            recv_piped_stdin(&probe_rx, &data_rx, STDIN_PIPE_GRACE, cli.prompt.is_some())?;
    }

    let prompt = match cli.prompt {
        Some(mut p) => {
            if !stdin_prompt.is_empty() {
                p.push_str("\n\n");
                p.push_str(&stdin_prompt);
            }
            p
        }
        None => stdin_prompt,
    };

    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        // No prompt and no command, print help
        let mut cmd = Cli::command();
        if let Some(version) = cmd.get_version() {
            println!("ask-bridge {}", version);
        } else {
            println!("ask-bridge {}", env!("CARGO_PKG_VERSION"));
        }
        cmd.print_help()?;
        println!();
        std::process::exit(0);
    }

    if let Some((session_provider, session_url)) = &session_target {
        if let Err(e) = open_url_tab(
            &config_path,
            *session_provider,
            session_url,
            is_headless,
            command_verbose,
        ) {
            eprintln!(
                "Error opening {} session: {}",
                session_provider.display_name(),
                e
            );
            std::process::exit(1);
        }
    } else if let Err(e) = ensure_provider_tab(
        &config_path,
        provider,
        cli.new,
        is_headless,
        command_verbose,
    ) {
        eprintln!("Error ensuring {} tab: {}", provider.display_name(), e);
        std::process::exit(1);
    }

    // Show attached images in the terminal before sending
    if !cli.images.is_empty() {
        for img_path in &cli.images {
            display_image_in_terminal(img_path);
        }
    }

    // Verify login
    match check_login_status(&config_path, provider, command_verbose) {
        Ok(LoginState::LoggedOut) => {
            if provider == Provider::M365Copilot {
                eprintln!(
                    "\nError: authentication: You are not logged in to Microsoft 365 Copilot."
                );
                eprintln!(
                    "Run `ask-bridge --provider m365 login` and complete Microsoft Entra sign-in manually.\n"
                );
            } else {
                eprintln!(
                    "\nError: You are not logged in to {}.",
                    provider.display_name()
                );
                eprintln!(
                    "Please run `ask-bridge --provider {} login` to log in manually first, and then run your query again.\n",
                    provider
                );
            }
            std::process::exit(1);
        }
        Ok(LoginState::Unknown) if provider == Provider::M365Copilot => {
            eprintln!(
                "Error: authentication: Could not safely confirm the Microsoft 365 Copilot login state."
            );
            eprintln!(
                "Run `ask-bridge --provider m365 login` in a visible browser and complete any Microsoft Entra or Conditional Access verification."
            );
            std::process::exit(1);
        }
        Ok(LoginState::Unknown) => {
            eprintln!(
                "Warning: Could not confirm the {} account menu. Attempting to proceed...",
                provider.display_name()
            );
        }
        Ok(LoginState::LoggedIn) => {}
        Err(e) if provider == Provider::M365Copilot => {
            eprintln!(
                "Error: authentication: Failed to verify the Microsoft 365 Copilot login state: {}",
                e
            );
            eprintln!("Run `ask-bridge --provider m365 login` in a visible browser and retry.");
            std::process::exit(1);
        }
        Err(e) if command_verbose => {
            eprintln!(
                "Warning: Failed to verify login status: {}. Attempting to proceed...",
                e
            );
        }
        Err(_) => {}
    }

    if let Some((session_provider, session_url)) = &session_target
        && let Err(error) = verify_resumed_session(&config_path, *session_provider, session_url)
    {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }

    // Switch model if requested (before uploading attachments / typing the prompt)
    if let Some(m) = &selection_plan.model
        && let Err(e) = switch_model(&config_path, provider, m, command_verbose)
    {
        eprintln!("Error switching model: {}", e);
        std::process::exit(1);
    }
    if let Some(reasoning) = selection_plan.reasoning
        && let Err(e) = switch_reasoning(&config_path, provider, reasoning, command_verbose)
    {
        eprintln!("Error switching reasoning: {}", e);
        std::process::exit(1);
    }

    // Upload any attached images/files before counting messages (so the UI is ready)
    if (!cli.images.is_empty() || !cli.files.is_empty())
        && let Err(e) = upload_attachments_to_provider(
            &config_path,
            provider,
            &cli.images,
            &cli.files,
            command_verbose,
        )
    {
        eprintln!("Error attaching images/files: {}", e);
        std::process::exit(1);
    }

    // Get initial number of assistant messages before submitting the prompt
    let assistant_selector = serde_json::to_string(provider.assistant_selector())
        .map_err(|e| format!("Failed to serialize assistant selector: {}", e))?;
    let count_res = call_mcp_tool(
        &config_path,
        "evaluate_script",
        serde_json::json!({
            "function": format!("() => document.querySelectorAll({}).length", assistant_selector)
        }),
    )?;
    let initial_assistant_count = parse_script_result(&count_res)
        .ok()
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    if command_verbose {
        println!("Setting prompt text and submitting...");
    }
    let status = submit_prompt_to_provider(&config_path, provider, &prompt, command_verbose)
        .map_err(|e| format!("Text entry or submission failed: {}", e))?;

    if command_verbose {
        println!("Prompt submitted successfully: {}", status);
    }

    if command_verbose {
        println!("Waiting for {} response...", provider.display_name());
    }

    let mut last_markdown = String::new();
    let mut finished = false;
    let mut wait_cycles = 0;
    let mut completion_tracker = ResponseCompletionTracker::default();
    let requires_text_stability = provider == Provider::M365Copilot;
    let spinner_frames = vec!["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut spinner_idx = 0;

    let max_wait_cycles: usize =
        usize::try_from(cli.timeout.saturating_mul(10)).unwrap_or(usize::MAX);
    while !finished && wait_cycles < max_wait_cycles {
        // Max wait time: timeout seconds (timeout * 10 * 100ms)
        if is_terminal {
            let frame = spinner_frames[spinner_idx % spinner_frames.len()];
            print!(
                "\r\x1b[1;36m{}\x1b[0m 正在等待 {} 回應...",
                frame,
                provider.display_name()
            );
            io::stdout().flush()?;
            spinner_idx += 1;
        }

        if wait_cycles % 5 == 0 {
            let stop_selectors = provider.stop_button_selectors_json();
            let assistant_selector = serde_json::to_string(provider.assistant_selector())
                .map_err(|e| format!("Failed to serialize assistant selector: {}", e))?;
            let content_selector = serde_json::to_string(provider.response_content_selector())
                .map_err(|e| format!("Failed to serialize response content selector: {}", e))?;
            let response_check_js = r#"() => {
                    const stopSelectors = __STOP_SELECTORS__;
                    const contentSelector = __CONTENT_SELECTOR__;
                    const isVisible = (el) => {
                        if (!el || el.disabled || el.getAttribute('aria-disabled') === 'true') return false;
                        const style = window.getComputedStyle(el);
                        if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
                        const rect = el.getBoundingClientRect();
                        return rect.width > 0 && rect.height > 0;
                    };
                    const stopButton = stopSelectors.map((selector) => document.querySelector(selector)).find(isVisible);
                    const messages = document.querySelectorAll(__ASSISTANT_SELECTOR__);
                    const isNew = messages.length > __INITIAL_COUNT__;
                    const latest = messages[messages.length - 1];
                    const contentRoot = latest
                        ? (contentSelector ? (latest.querySelector(contentSelector) || latest) : latest)
                        : null;
                    const responseText = (contentRoot?.innerText || contentRoot?.textContent || '').trim();
                    let contentHash = 2166136261;
                    for (let index = 0; index < responseText.length; index++) {
                        contentHash ^= responseText.charCodeAt(index);
                        contentHash = Math.imul(contentHash, 16777619);
                    }
                    
                    if (isVisible(stopButton)) {
                        return {
                            status: "generating",
                            isNew: isNew,
                            contentLength: responseText.length,
                            contentHash: contentHash >>> 0
                        };
                    }
                    
                    if (isNew) {
                        return {
                            status: "done",
                            isNew: isNew,
                            contentLength: responseText.length,
                            contentHash: contentHash >>> 0
                        };
                    }
                    
                    return {
                        status: "waiting",
                        isNew: isNew,
                        contentLength: responseText.length,
                        contentHash: contentHash >>> 0
                    };
                }"#
            .replace("__STOP_SELECTORS__", stop_selectors)
            .replace("__CONTENT_SELECTOR__", &content_selector)
            .replace("__ASSISTANT_SELECTOR__", &assistant_selector)
            .replace("__INITIAL_COUNT__", &initial_assistant_count.to_string());
            let check_res = match call_mcp_tool(
                &config_path,
                "evaluate_script",
                serde_json::json!({
                    "function": response_check_js
                }),
            ) {
                Ok(res) => res,
                Err(e) => {
                    if command_verbose {
                        eprintln!(
                            "Warning: Failed to poll {} response: {}",
                            provider.display_name(),
                            e
                        );
                    }
                    thread::sleep(Duration::from_millis(100));
                    wait_cycles += 1;
                    continue;
                }
            };

            if let Ok(parsed) = parse_script_result(&check_res) {
                let status = parsed["status"].as_str().unwrap_or("waiting");
                let is_new = parsed["isNew"].as_bool().unwrap_or(false);
                let content_length = parsed["contentLength"].as_u64().unwrap_or(0);
                let content_hash = parsed["contentHash"].as_u64().unwrap_or(0);
                finished = completion_tracker.observe(
                    status,
                    is_new,
                    content_length,
                    content_hash,
                    requires_text_stability,
                );
            }
        }

        thread::sleep(Duration::from_millis(100));
        wait_cycles += 1;
    }

    if is_terminal {
        print!("\r\x1b[K");
        io::stdout().flush()?;
    }

    if !finished {
        if provider == Provider::M365Copilot {
            return Err(format!(
                "Microsoft 365 Copilot response did not complete within {} seconds",
                cli.timeout
            )
            .into());
        }
        eprintln!(
            "\nWarning: Output stream did not complete within the timeout period ({} seconds).",
            cli.timeout
        );
    }

    if finished {
        if command_verbose {
            println!(
                "Copying final response from {} toolbar...",
                provider.display_name()
            );
        }
        last_markdown =
            copy_latest_markdown_for_request(&config_path, provider, cli.image_output.as_deref())
                .map_err(|e| {
                format!(
                    "Failed to copy response from {} toolbar or DOM: {}",
                    provider.display_name(),
                    e
                )
            })?;
    }

    if let Err(e) = render_markdown(&last_markdown, use_glow) {
        eprintln!("Error rendering Markdown: {}", e);
    }

    if finished
        && provider.capabilities().image_download
        && let Err(error) = download_images_from_latest_message(
            &config_path,
            provider,
            cli.image_output.as_deref(),
            command_verbose,
        )
    {
        eprintln!("Error downloading images: {error}");
        if provider == Provider::M365Copilot && cli.image_output.is_some() {
            std::process::exit(1);
        }
    }

    // Print the URL link of the current conversation thread
    let url_opt = call_mcp_tool(
        &config_path,
        "evaluate_script",
        serde_json::json!({
            "function": "() => window.location.href"
        }),
    )
    .ok()
    .and_then(|url_val| parse_script_result(&url_val).ok())
    .and_then(|u| u.as_str().map(|s| s.to_string()));

    if let Some(url) = url_opt {
        if is_terminal {
            println!("\n🌐 \x1b[1mThread Link:\x1b[0m \x1b[4;36m{}\x1b[0m", url);
        } else {
            println!("\nThread Link: {}", url);
        }
    }

    if let Some(ref output_path) = cli.output {
        if let Err(e) = std::fs::write(output_path, &last_markdown) {
            eprintln!("Error writing output file: {}", e);
        } else if command_verbose {
            println!("Successfully wrote Markdown response to {}", output_path);
        }
    }

    Ok(())
}
