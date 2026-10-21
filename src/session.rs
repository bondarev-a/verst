use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::places;
use crate::seams::{self, PanelSide};
use crate::zoom::{self, BASE_PT};

pub const OPEN_LIMIT_DEFAULT: u32 = 5;
pub const OPEN_LIMIT_MIN: u32 = 1;
pub const OPEN_LIMIT_MAX: u32 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Edit,
    Preview,
}

impl ViewMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Preview => "preview",
        }
    }

    pub fn parse(value: &str) -> Self {
        if value == "preview" {
            Self::Preview
        } else {
            Self::Edit
        }
    }
}

impl Serialize for ViewMode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ViewMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(Self::parse(&value))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    #[serde(default)]
    pub pins: Vec<PathBuf>,
    #[serde(default)]
    pub active_pin: Option<usize>,
    #[serde(default)]
    pub open_files: Vec<PathBuf>,
    #[serde(default = "default_panes")]
    pub pane_paths: Vec<Option<PathBuf>>,
    #[serde(default)]
    pub focused_pane: usize,
    #[serde(default)]
    pub split: bool,
    #[serde(default = "default_zoom")]
    pub zoom_pt: i32,
    #[serde(default = "default_theme")]
    pub theme: String,
    /// В каком виде открывается следующий файл.
    #[serde(default)]
    pub open_mode: ViewMode,
    /// Текущий вид открытого файла, отдельно от настройки по умолчанию.
    #[serde(default)]
    pub view_mode: ViewMode,
    #[serde(default = "default_window_width")]
    pub window_width: i32,
    #[serde(default = "default_window_height")]
    pub window_height: i32,
    #[serde(default)]
    pub left_page: u32,
    #[serde(default)]
    pub right_page: u32,
    /// Устойчивое имя левой вкладки: disk, workspace, open, history.
    #[serde(default)]
    pub left_panel: String,
    /// Устойчивое имя правой вкладки: outline, search, backlinks (в меню «Ссылки»), graph.
    #[serde(default)]
    pub right_panel: String,
    #[serde(default)]
    pub carets: Vec<CaretPlace>,
    /// Закреплённые файлы, в порядке закрепления: новые сверху.
    #[serde(default)]
    pub pinned_files: Vec<PathBuf>,
    /// Недавно открытые файлы, новые сверху.
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
    /// Сколько файлов держать в списке открытых. Лишние закрываются с самых давних.
    #[serde(default = "default_open_limit")]
    pub open_limit: u32,
    /// Перенос длинных строк по ширине редактора.
    #[serde(default)]
    pub word_wrap: bool,
    /// В каком виде оставался каждый файл: редактирование или просмотр.
    #[serde(default)]
    pub file_modes: Vec<FileMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileMode {
    pub path: PathBuf,
    pub mode: ViewMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaretPlace {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
}

fn default_panes() -> Vec<Option<PathBuf>> {
    vec![None, None]
}

fn default_zoom() -> i32 {
    BASE_PT
}

fn default_theme() -> String {
    "Adwaita".to_string()
}

fn default_window_width() -> i32 {
    1280
}

fn default_window_height() -> i32 {
    800
}

fn default_open_limit() -> u32 {
    OPEN_LIMIT_DEFAULT
}

impl Default for Session {
    fn default() -> Self {
        Self {
            pins: Vec::new(),
            active_pin: None,
            open_files: Vec::new(),
            pane_paths: default_panes(),
            focused_pane: 0,
            split: false,
            zoom_pt: BASE_PT,
            theme: default_theme(),
            open_mode: ViewMode::Edit,
            view_mode: ViewMode::Edit,
            window_width: default_window_width(),
            window_height: default_window_height(),
            left_page: 0,
            right_page: 0,
            left_panel: "disk".to_string(),
            right_panel: "outline".to_string(),
            carets: Vec::new(),
            pinned_files: Vec::new(),
            recent_files: Vec::new(),
            open_limit: OPEN_LIMIT_DEFAULT,
            word_wrap: false,
            file_modes: Vec::new(),
        }
    }
}

impl Session {
    pub fn config_path() -> PathBuf {
        config_dir().join("session.json")
    }

    pub fn active_pin_path(&self) -> Option<&Path> {
        self.active_pin
            .and_then(|i| self.pins.get(i))
            .map(PathBuf::as_path)
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => return Self::default(),
        };
        Self::from_json(&text)
            .map(|mut session| {
                session.retain_present();
                session
            })
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string());
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, text)?;
        fs::rename(tmp, path)
    }

    pub fn from_json(text: &str) -> Option<Self> {
        let mut session: Session = serde_json::from_str(text).ok()?;
        session.normalize();
        Some(session)
    }

    pub fn normalize(&mut self) {
        places::dedupe(&mut self.pins);
        places::dedupe(&mut self.open_files);
        places::dedupe(&mut self.pinned_files);
        places::dedupe(&mut self.recent_files);
        self.recent_files.truncate(places::RECENT_CAP);
        if self.pane_paths.len() < 2 {
            self.pane_paths.resize(2, None);
        } else if self.pane_paths.len() > 2 {
            self.pane_paths.truncate(2);
        }
        if self.focused_pane > 1 {
            self.focused_pane = 0;
        }
        if self.active_pin.is_some_and(|i| i >= self.pins.len()) {
            self.active_pin = if self.pins.is_empty() { None } else { Some(0) };
        }
        self.zoom_pt = zoom::clamp_pt(self.zoom_pt);
        if self.theme.is_empty() {
            self.theme = default_theme();
        }
        self.left_panel = seams::canonical_panel(PanelSide::Left, &self.left_panel, self.left_page);
        self.right_panel =
            seams::canonical_panel(PanelSide::Right, &self.right_panel, self.right_page);
        self.left_page = seams::panel_index(&self.left_panel).unwrap_or(0);
        self.right_page = seams::panel_index(&self.right_panel).unwrap_or(0);
        self.window_width = self.window_width.clamp(800, 8000);
        self.window_height = self.window_height.clamp(500, 8000);
        dedupe_carets(&mut self.carets);
        for pane in self.pane_paths.clone() {
            if let Some(path) = pane {
                if !self.open_files.iter().any(|open| open == &path) {
                    self.open_files.push(path);
                }
            }
        }
        self.open_limit = self.open_limit.clamp(OPEN_LIMIT_MIN, OPEN_LIMIT_MAX);
        trim_open_files(self);
        dedupe_file_modes(&mut self.file_modes);
        if self.file_modes.is_empty() {
            let mode = self.view_mode;
            let paths = self.open_files.clone();
            for path in paths {
                self.remember_mode(path, mode);
            }
        }
    }

    pub fn mode_of(&self, path: &Path) -> Option<ViewMode> {
        self.file_modes
            .iter()
            .rev()
            .find(|item| item.path == path)
            .map(|item| item.mode)
    }

    pub fn remember_mode(&mut self, path: PathBuf, mode: ViewMode) {
        self.file_modes.retain(|item| item.path != path);
        self.file_modes.push(FileMode { path, mode });
        const CAP: usize = 200;
        if self.file_modes.len() > CAP {
            let extra = self.file_modes.len() - CAP;
            self.file_modes.drain(0..extra);
        }
    }

    pub fn retain_present(&mut self) {
        self.pins.retain(|path| path.is_dir());
        self.open_files.retain(|path| path.is_file());
        for pane in &mut self.pane_paths {
            if pane.as_ref().is_some_and(|path| !path.is_file()) {
                *pane = None;
            }
        }
        self.carets.retain(|place| place.path.is_file());
        self.file_modes.retain(|item| item.path.is_file());
        self.pinned_files.retain(|path| path.is_file());
        self.recent_files.retain(|path| path.is_file());
        if self.active_pin.is_some_and(|i| i >= self.pins.len()) {
            self.active_pin = if self.pins.is_empty() { None } else { Some(0) };
        }
    }
}

fn trim_open_files(session: &mut Session) {
    let limit = session.open_limit as usize;
    let panes: Vec<PathBuf> = session.pane_paths.iter().flatten().cloned().collect();
    while session.open_files.len() > limit {
        let Some(index) = session
            .open_files
            .iter()
            .position(|open| !panes.iter().any(|pane| pane == open))
        else {
            break;
        };
        session.open_files.remove(index);
    }
}

fn dedupe_file_modes(modes: &mut Vec<FileMode>) {
    let mut seen = std::collections::HashSet::new();
    modes.reverse();
    modes.retain(|item| seen.insert(item.path.clone()));
    modes.reverse();
}

fn dedupe_carets(carets: &mut Vec<CaretPlace>) {
    let mut seen = std::collections::HashSet::new();
    carets.reverse();
    carets.retain(|place| seen.insert(place.path.clone()));
    carets.reverse();
}

pub fn config_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("text_workspace");
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".config").join("text_workspace")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_use_defaults() {
        let session = Session::from_json("{}").unwrap();
        assert!(session.pins.is_empty());
        assert_eq!(session.zoom_pt, BASE_PT);
        assert_eq!(session.theme, "Adwaita");
        assert_eq!(session.open_mode, ViewMode::Edit);
        assert_eq!(session.view_mode, ViewMode::Edit);
        assert_eq!(session.window_width, 1280);
        assert_eq!(session.window_height, 800);
        assert_eq!(session.pane_paths.len(), 2);
        assert_eq!(session.left_panel, "disk");
        assert_eq!(session.right_panel, "outline");
        assert!(session.carets.is_empty());
        assert!(session.pinned_files.is_empty());
        assert!(session.recent_files.is_empty());
        assert_eq!(session.open_limit, OPEN_LIMIT_DEFAULT);
        assert!(!session.word_wrap);
        assert!(session.file_modes.is_empty());
        let session = Session::from_json(r#"{"word_wrap":true}"#).unwrap();
        assert!(session.word_wrap);
    }

    #[test]
    fn history_page_is_kept() {
        let session = Session::from_json(r#"{"left_page":3}"#).unwrap();
        assert_eq!(session.left_panel, "history");
        assert_eq!(session.left_page, 3);
        let session = Session::from_json(r#"{"left_page":9}"#).unwrap();
        assert_eq!(session.left_panel, "disk");
        let session = Session::from_json(r#"{"left_panel":"open","left_page":0}"#).unwrap();
        assert_eq!(session.left_panel, "open");
        assert_eq!(session.left_page, 2);
    }

    #[test]
    fn unknown_mode_falls_back_to_edit() {
        let session = Session::from_json(
            r#"{"view_mode":"both","open_mode":"both","pane_layout":"vertical","source_first":false}"#,
        )
        .unwrap();
        assert_eq!(session.view_mode, ViewMode::Edit);
        assert_eq!(session.open_mode, ViewMode::Edit);
    }

    #[test]
    fn zoom_is_clamped_on_load() {
        let session = Session::from_json(r#"{"zoom_pt": 400}"#).unwrap();
        assert_eq!(session.zoom_pt, zoom::MAX_PT);
    }

    #[test]
    fn open_limit_defaults_and_drops_oldest() {
        let session = Session::from_json(r#"{"open_limit": 0}"#).unwrap();
        assert_eq!(session.open_limit, OPEN_LIMIT_MIN);
        let session = Session::from_json(r#"{"open_limit": 500}"#).unwrap();
        assert_eq!(session.open_limit, OPEN_LIMIT_MAX);
        let session = Session::from_json(
            r#"{"open_limit":2,"open_files":["/a","/b","/c"],"pane_paths":["/c",null]}"#,
        )
        .unwrap();
        assert_eq!(
            session.open_files,
            vec![PathBuf::from("/b"), PathBuf::from("/c")]
        );
    }

    #[test]
    fn each_file_keeps_its_view_mode() {
        let mut session = Session::default();
        session.remember_mode(PathBuf::from("/a.md"), ViewMode::Preview);
        session.remember_mode(PathBuf::from("/b.md"), ViewMode::Edit);
        session.remember_mode(PathBuf::from("/a.md"), ViewMode::Edit);
        assert_eq!(session.mode_of(Path::new("/a.md")), Some(ViewMode::Edit));
        assert_eq!(session.mode_of(Path::new("/b.md")), Some(ViewMode::Edit));
        assert_eq!(session.mode_of(Path::new("/c.md")), None);
        let session =
            Session::from_json(r#"{"open_files":["/a.md"],"view_mode":"preview"}"#).unwrap();
        assert_eq!(session.mode_of(Path::new("/a.md")), Some(ViewMode::Preview));
        let session =
            Session::from_json(r#"{"file_modes":[{"path":"/a.md","mode":"preview"}]}"#).unwrap();
        assert_eq!(session.mode_of(Path::new("/a.md")), Some(ViewMode::Preview));
        assert_eq!(session.mode_of(Path::new("/missing.md")), None);
    }
}
