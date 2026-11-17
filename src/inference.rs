//! Доступ к инференсу. Поля те же, что у AIGent: провайдер, адрес, ключ
//! или переменная окружения, модель. Каталога профилей нет — один доступ.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::session;

pub const KEY_ENV: &str = "LLM_API_KEY";
pub const SELECTION_LIMIT: usize = 12_000;
const ERROR_CHARS: usize = 600;
const ANTHROPIC_MAX_TOKENS: u32 = 8192;
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    OpenaiCompatible,
    Anthropic,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPlace {
    File,
    Env,
    Missing,
}

pub fn key_place_for(stored: bool, env_set: bool) -> KeyPlace {
    if stored {
        KeyPlace::File
    } else if env_set {
        KeyPlace::Env
    } else {
        KeyPlace::Missing
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Access {
    #[serde(default)]
    pub provider: Provider,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_key_env")]
    pub api_key_env: String,
    #[serde(default)]
    pub model: String,
}

fn default_key_env() -> String {
    KEY_ENV.to_string()
}

impl Default for Access {
    fn default() -> Self {
        Self {
            provider: Provider::OpenaiCompatible,
            base_url: String::new(),
            api_key: String::new(),
            api_key_env: default_key_env(),
            model: String::new(),
        }
    }
}

impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Access")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "empty"
                } else {
                    "set"
                },
            )
            .field("api_key_env", &self.api_key_env)
            .field("model", &self.model)
            .finish()
    }
}

pub struct AccessDraft {
    pub provider: Provider,
    pub base_url: String,
    pub api_key: String,
    pub clear_key: bool,
    pub api_key_env: String,
    pub model: String,
}

impl Access {
    pub fn path() -> PathBuf {
        session::config_dir().join("inference.json")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let text = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string());
        let tmp = path.with_extension("json.tmp");
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            file.write_all(text.as_bytes())?;
        }
        fs::rename(&tmp, path)?;
        let mut perms = fs::metadata(path)?.permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)
    }

    pub fn apply_draft(&mut self, draft: AccessDraft) {
        self.provider = draft.provider;
        self.base_url = draft.base_url.trim().to_string();
        self.model = draft.model.trim().to_string();
        let env = draft.api_key_env.trim();
        self.api_key_env = if env.is_empty() {
            default_key_env()
        } else {
            env.to_string()
        };
        if draft.clear_key {
            self.api_key.clear();
        } else {
            let typed = draft.api_key.trim();
            if !typed.is_empty() {
                self.api_key = typed.to_string();
            }
        }
    }

    pub fn resolved_api_key(&self) -> Option<String> {
        let stored = self.api_key.trim();
        if !stored.is_empty() {
            return Some(stored.to_string());
        }
        std::env::var(self.api_key_env.trim())
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    pub fn key_place(&self) -> KeyPlace {
        key_place_for(
            !self.api_key.trim().is_empty(),
            env_set(self.api_key_env.trim()),
        )
    }
}

fn env_set(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    std::env::var(name)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskFail {
    EmptySelection,
    EmptyQuestion,
    ProviderOff,
    NoAddress,
    NoModel,
    NoKey { env: String },
    TooLong { limit: usize },
}

impl std::fmt::Display for AskFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySelection => write!(f, "Выделите фрагмент"),
            Self::EmptyQuestion => write!(f, "Напишите, что сделать с выделением"),
            Self::ProviderOff => write!(f, "Инференс выключен в настройках"),
            Self::NoAddress => write!(f, "В настройках не задан адрес инференса"),
            Self::NoModel => write!(f, "В настройках не задана модель"),
            Self::NoKey { env } => {
                write!(f, "Нет ключа. Впишите его в настройках или задайте {env}")
            }
            Self::TooLong { limit } => {
                write!(f, "Выделение длиннее {limit} знаков")
            }
        }
    }
}

pub const SYSTEM_PROMPT: &str = "\
Ты помогаешь с выделенным фрагментом текста. \
Опирайся только на этот фрагмент. \
Если просят его изменить, верни только новый текст фрагмента, без кавычек и пояснений.";

pub fn user_message(selection: &str, question: &str) -> String {
    format!("Выделенный фрагмент:\n{selection}\n\nЗапрос:\n{question}")
}

pub fn chat_url(provider: Provider, base_url: &str) -> Result<String, AskFail> {
    if provider == Provider::None {
        return Err(AskFail::ProviderOff);
    }
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(AskFail::NoAddress);
    }
    match provider {
        Provider::OpenaiCompatible => Ok(format!("{base}/chat/completions")),
        Provider::Anthropic => {
            let root = base.strip_suffix("/v1").unwrap_or(base);
            Ok(format!("{root}/v1/messages"))
        }
        Provider::None => Err(AskFail::ProviderOff),
    }
}

pub fn request_body(
    provider: Provider,
    model: &str,
    system: &str,
    user: &str,
) -> serde_json::Value {
    match provider {
        Provider::Anthropic => serde_json::json!({
            "model": model,
            "max_tokens": ANTHROPIC_MAX_TOKENS,
            "system": system,
            "messages": [{"role": "user", "content": user}],
            "temperature": 0.2
        }),
        _ => serde_json::json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "temperature": 0.2
        }),
    }
}

pub struct Prepared {
    pub url: String,
    pub provider: Provider,
    pub body: serde_json::Value,
    api_key: String,
}

pub fn prepare(access: &Access, selection: &str, question: &str) -> Result<Prepared, AskFail> {
    let selection = selection.trim();
    if selection.is_empty() {
        return Err(AskFail::EmptySelection);
    }
    let question = question.trim();
    if question.is_empty() {
        return Err(AskFail::EmptyQuestion);
    }
    let chars = selection.chars().count();
    if chars > SELECTION_LIMIT {
        return Err(AskFail::TooLong {
            limit: SELECTION_LIMIT,
        });
    }
    if access.provider == Provider::None {
        return Err(AskFail::ProviderOff);
    }
    let url = chat_url(access.provider, &access.base_url)?;
    let model = access.model.trim();
    if model.is_empty() {
        return Err(AskFail::NoModel);
    }
    let Some(api_key) = access.resolved_api_key() else {
        return Err(AskFail::NoKey {
            env: access.api_key_env.clone(),
        });
    };
    let user = user_message(selection, question);
    Ok(Prepared {
        url,
        provider: access.provider,
        body: request_body(access.provider, model, SYSTEM_PROMPT, &user),
        api_key,
    })
}

pub fn parse_reply(provider: Provider, body: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "Ответ инференса не разобран".to_string())?;
    let text = match provider {
        Provider::Anthropic => anthropic_text(&value),
        _ => openai_text(&value),
    };
    Ok(text)
}

pub fn complete(prepared: &Prepared) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        .timeout_write(Duration::from_secs(30))
        .build();
    let request = agent.post(&prepared.url);
    let request = match prepared.provider {
        Provider::Anthropic => request
            .set("x-api-key", &prepared.api_key)
            .set("anthropic-version", ANTHROPIC_VERSION),
        _ => request.set("Authorization", &format!("Bearer {}", prepared.api_key)),
    };
    let request = request.set("Content-Type", "application/json");
    match request.send_json(&prepared.body) {
        Ok(response) => {
            let body = response
                .into_string()
                .map_err(|err| scrub(&err.to_string(), &prepared.api_key))?;
            let text = parse_reply(prepared.provider, &body)
                .map_err(|err| scrub(&err, &prepared.api_key))?;
            let text = text.trim().to_string();
            if text.is_empty() {
                Err("В ответе нет текста".to_string())
            } else {
                Ok(text)
            }
        }
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            Err(scrub(
                &format!("Инференс ответил {code}: {}", error_detail(&body)),
                &prepared.api_key,
            ))
        }
        Err(ureq::Error::Transport(err)) => Err(scrub(
            &format!("Нет связи с инференсом: {err}"),
            &prepared.api_key,
        )),
    }
}

fn openai_text(value: &serde_json::Value) -> String {
    let content = &value["choices"][0]["message"]["content"];
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part["text"].as_str().or_else(|| part.as_str()))
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn anthropic_text(value: &serde_json::Value) -> String {
    value["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block["type"].as_str() == Some("text"))
                .filter_map(|block| block["text"].as_str())
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn error_detail(body: &str) -> String {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value["error"]["message"]
                .as_str()
                .or_else(|| value["error"].as_str())
                .map(str::to_string)
        });
    let text = message.unwrap_or_else(|| body.trim().to_string());
    match text.char_indices().nth(ERROR_CHARS) {
        Some((index, _)) => format!("{}…", &text[..index]),
        None => text,
    }
}

const LOG_LIMIT: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exchange {
    pub question: String,
    pub selection: String,
    pub answer: String,
}

pub fn exchange_path() -> PathBuf {
    session::config_dir().join("ask_log.json")
}

pub fn load_exchanges() -> Vec<Exchange> {
    load_exchanges_from(&exchange_path())
}

pub fn load_exchanges_from(path: &Path) -> Vec<Exchange> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn remember_exchange(item: Exchange) {
    let _ = remember_exchange_at(&exchange_path(), item);
}

pub fn remember_exchange_at(path: &Path, item: Exchange) -> io::Result<()> {
    let mut items = load_exchanges_from(path);
    items.insert(0, item);
    items.truncate(LOG_LIMIT);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let text = serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string());
    let tmp = path.with_extension("json.tmp");
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
    }
    fs::rename(&tmp, path)?;
    let mut perms = fs::metadata(path)?.permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)
}

fn scrub(text: &str, secret: &str) -> String {
    let secret = secret.trim();
    if secret.len() < 8 {
        return text.to_string();
    }
    text.replace(secret, "••••")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> Access {
        Access {
            provider: Provider::OpenaiCompatible,
            base_url: "https://example.test/v1".into(),
            api_key: "sk-test-key".into(),
            api_key_env: KEY_ENV.into(),
            model: "demo-model".into(),
        }
    }

    #[test]
    fn stored_key_wins_over_env_name() {
        let mut access = ready();
        access.api_key_env = "THIS_VAR_IS_NOT_SET".into();
        assert_eq!(access.resolved_api_key().as_deref(), Some("sk-test-key"));
        assert_eq!(access.key_place(), KeyPlace::File);
    }

    #[test]
    fn empty_key_can_come_from_env() {
        let mut access = ready();
        access.api_key.clear();
        access.api_key_env = "TEXT_WORKSPACE_TEST_LLM_KEY".into();
        std::env::set_var("TEXT_WORKSPACE_TEST_LLM_KEY", "env-secret-value");
        assert_eq!(
            access.resolved_api_key().as_deref(),
            Some("env-secret-value")
        );
        assert_eq!(access.key_place(), KeyPlace::Env);
        std::env::remove_var("TEXT_WORKSPACE_TEST_LLM_KEY");
    }

    #[test]
    fn blank_draft_keeps_key_and_clear_removes_it() {
        let mut access = ready();
        access.apply_draft(AccessDraft {
            provider: Provider::Anthropic,
            base_url: " https://anthropic.test/v1 ".into(),
            api_key: "  ".into(),
            clear_key: false,
            api_key_env: "".into(),
            model: " claude ".into(),
        });
        assert_eq!(access.api_key, "sk-test-key");
        assert_eq!(access.api_key_env, KEY_ENV);
        assert_eq!(access.base_url, "https://anthropic.test/v1");
        assert_eq!(access.model, "claude");
        assert_eq!(access.provider, Provider::Anthropic);
        access.apply_draft(AccessDraft {
            provider: Provider::Anthropic,
            base_url: access.base_url.clone(),
            api_key: "typed-but-cleared".into(),
            clear_key: true,
            api_key_env: access.api_key_env.clone(),
            model: access.model.clone(),
        });
        assert!(access.api_key.is_empty());
    }

    #[test]
    fn openai_and_anthropic_urls() {
        assert_eq!(
            chat_url(Provider::OpenaiCompatible, "https://example.test/v1/").unwrap(),
            "https://example.test/v1/chat/completions"
        );
        assert_eq!(
            chat_url(Provider::Anthropic, "https://api.anthropic.com/v1").unwrap(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            chat_url(Provider::Anthropic, "https://api.anthropic.com").unwrap(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            chat_url(Provider::None, "https://example.test"),
            Err(AskFail::ProviderOff)
        );
        assert_eq!(
            chat_url(Provider::OpenaiCompatible, "  "),
            Err(AskFail::NoAddress)
        );
    }

    #[test]
    fn prepare_sends_only_the_selection() {
        let prepared = prepare(&ready(), "  абзац  ", "сделай короче").unwrap();
        let user = prepared.body["messages"][1]["content"].as_str().unwrap();
        assert_eq!(
            user,
            "Выделенный фрагмент:\nабзац\n\nЗапрос:\nсделай короче"
        );
        assert!(!user.contains('/'));
        assert_eq!(prepared.body["model"], "demo-model");
        assert_eq!(prepared.url, "https://example.test/v1/chat/completions");
    }

    #[test]
    fn prepare_refuses_incomplete_access() {
        let mut access = ready();
        assert!(prepare(&access, "   ", "вопрос").is_err());
        assert!(matches!(
            prepare(&access, "текст", "  "),
            Err(AskFail::EmptyQuestion)
        ));
        access.model.clear();
        assert!(matches!(
            prepare(&access, "текст", "вопрос"),
            Err(AskFail::NoModel)
        ));
        access.model = "demo-model".into();
        access.api_key.clear();
        access.api_key_env = "TEXT_WORKSPACE_ABSENT_KEY".into();
        assert!(matches!(
            prepare(&access, "текст", "вопрос"),
            Err(AskFail::NoKey { .. })
        ));
        let long = "я".repeat(SELECTION_LIMIT + 1);
        assert!(matches!(
            prepare(&ready(), &long, "вопрос"),
            Err(AskFail::TooLong { .. })
        ));
    }

    #[test]
    fn replies_parse_both_providers() {
        let openai = r#"{"choices":[{"message":{"content":"короче"}}]}"#;
        assert_eq!(
            parse_reply(Provider::OpenaiCompatible, openai).unwrap(),
            "короче"
        );
        let parts = r#"{"choices":[{"message":{"content":[{"type":"text","text":"а"},{"type":"text","text":"б"}]}}]}"#;
        assert_eq!(
            parse_reply(Provider::OpenaiCompatible, parts).unwrap(),
            "аб"
        );
        let anthropic =
            r#"{"content":[{"type":"thinking","thinking":"нет"},{"type":"text","text":"да"}]}"#;
        assert_eq!(parse_reply(Provider::Anthropic, anthropic).unwrap(), "да");
        assert!(parse_reply(Provider::OpenaiCompatible, "не json").is_err());
    }

    #[test]
    fn error_detail_prefers_message_and_scrubs_key() {
        assert_eq!(
            error_detail(r#"{"error":{"message":"bad model"}}"#),
            "bad model"
        );
        let secret = "sk-test-key";
        assert_eq!(
            scrub(&format!("failed {secret} here"), secret),
            "failed •••• here"
        );
        assert_eq!(scrub("short stays", "abc"), "short stays");
    }

    #[test]
    fn exchange_log_keeps_newest_first() {
        let dir =
            std::env::temp_dir().join(format!("text_workspace_ask_log_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ask_log.json");
        remember_exchange_at(
            &path,
            Exchange {
                question: "первый".into(),
                selection: "абзац".into(),
                answer: "ответ".into(),
            },
        )
        .unwrap();
        remember_exchange_at(
            &path,
            Exchange {
                question: "второй".into(),
                selection: "кусок".into(),
                answer: "ещё".into(),
            },
        )
        .unwrap();
        let items = load_exchanges_from(&path);
        assert_eq!(items[0].question, "второй");
        assert_eq!(items[1].selection, "абзац");
        assert_eq!(items[0].answer, "ещё");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_roundtrip_hides_key_from_debug_and_locks_file() {
        let dir =
            std::env::temp_dir().join(format!("text_workspace_inference_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("inference.json");
        let access = ready();
        access.save_to(&path).unwrap();
        let loaded = Access::load_from(&path);
        assert_eq!(loaded, access);
        let debug = format!("{access:?}");
        assert!(!debug.contains("sk-test-key"));
        assert!(debug.contains("set"));
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }
}
