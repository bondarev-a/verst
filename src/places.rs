use std::collections::HashSet;
use std::path::PathBuf;

pub const RECENT_CAP: usize = 40;
const NAV_CAP: usize = 80;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavPlace {
    File {
        path: PathBuf,
        line: u32,
        preview: bool,
    },
    Folder {
        path: PathBuf,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NavHistory {
    back: Vec<NavPlace>,
    forward: Vec<NavPlace>,
}

impl NavHistory {
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn leave(&mut self, here: NavPlace) {
        self.back.push(here);
        if self.back.len() > NAV_CAP {
            self.back.remove(0);
        }
        self.forward.clear();
    }

    pub fn back(&mut self, here: Option<NavPlace>) -> Option<NavPlace> {
        let dest = self.back.pop()?;
        if let Some(here) = here {
            self.forward.push(here);
        }
        Some(dest)
    }

    pub fn forward(&mut self, here: Option<NavPlace>) -> Option<NavPlace> {
        let dest = self.forward.pop()?;
        if let Some(here) = here {
            self.back.push(here);
        }
        Some(dest)
    }
}

pub fn remember(files: &mut Vec<PathBuf>, path: PathBuf) {
    files.retain(|item| item != &path);
    files.insert(0, path);
    files.truncate(RECENT_CAP);
}

pub fn toggle_pin(pins: &mut Vec<PathBuf>, path: PathBuf) -> bool {
    if let Some(index) = pins.iter().position(|item| item == &path) {
        pins.remove(index);
        false
    } else {
        pins.insert(0, path);
        true
    }
}

pub fn dedupe(files: &mut Vec<PathBuf>) {
    let mut seen = HashSet::new();
    files.retain(|path| seen.insert(path.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_moves_repeat_to_front_and_caps() {
        let mut files = Vec::new();
        remember(&mut files, PathBuf::from("/a"));
        remember(&mut files, PathBuf::from("/b"));
        remember(&mut files, PathBuf::from("/a"));
        assert_eq!(files, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        for index in 0..RECENT_CAP + 5 {
            remember(&mut files, PathBuf::from(format!("/n{index}")));
        }
        assert_eq!(files.len(), RECENT_CAP);
        assert_eq!(files[0], PathBuf::from(format!("/n{}", RECENT_CAP + 4)));
    }

    #[test]
    fn pin_toggles_and_puts_new_first() {
        let mut pins = Vec::new();
        assert!(toggle_pin(&mut pins, PathBuf::from("/a")));
        assert!(toggle_pin(&mut pins, PathBuf::from("/b")));
        assert_eq!(pins, vec![PathBuf::from("/b"), PathBuf::from("/a")]);
        assert!(!toggle_pin(&mut pins, PathBuf::from("/a")));
        assert_eq!(pins, vec![PathBuf::from("/b")]);
    }

    #[test]
    fn back_returns_to_the_place_we_left() {
        let mut nav = NavHistory::default();
        nav.leave(NavPlace::File {
            path: PathBuf::from("/a"),
            line: 2,
            preview: true,
        });
        nav.leave(NavPlace::Folder {
            path: PathBuf::from("/dir"),
        });
        let dest = nav
            .back(Some(NavPlace::File {
                path: PathBuf::from("/c"),
                line: 1,
                preview: false,
            }))
            .unwrap();
        assert_eq!(
            dest,
            NavPlace::Folder {
                path: PathBuf::from("/dir")
            }
        );
        assert!(nav.can_forward());
        let earlier = nav.back(None).unwrap();
        assert_eq!(
            earlier,
            NavPlace::File {
                path: PathBuf::from("/a"),
                line: 2,
                preview: true,
            }
        );
    }
}
