//! Folder-name completion for the download folder prompt.

use std::path::{MAIN_SEPARATOR, PathBuf};

/// Most suggestions kept for one directory.
const MAX_MATCHES: usize = 500;

/// Expands a leading `~` to the user's home folder.
pub fn expand_home(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix('~')
        && (rest.is_empty() || rest.starts_with(['/', '\\']))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest.trim_start_matches(['/', '\\']));
    }
    PathBuf::from(input)
}

/// Folder suggestions for a partly typed path, with shell-style Tab cycling.
#[derive(Debug)]
pub struct Completion {
    /// Everything up to and including the last separator the user typed.
    base: String,
    /// The folder name being typed after `base`.
    prefix: String,
    matches: Vec<String>,
    index: Option<usize>,
    /// False when the folder could not be read.
    readable: bool,
}

impl Completion {
    pub fn new(input: &str) -> Self {
        // `~` alone means "the home folder": list what is inside it.
        let input = if input == "~" {
            format!("~{MAIN_SEPARATOR}")
        } else {
            input.to_string()
        };
        let split = input.rfind(['/', '\\']).map_or(0, |i| i + 1);
        let (base, prefix) = input.split_at(split);

        let mut readable = true;
        let mut matches = Vec::new();
        if base.is_empty() {
            matches.extend(drives().filter(|d| starts_with_ignore_case(d, prefix)));
        }
        let dir = if base.is_empty() {
            PathBuf::from(".")
        } else {
            expand_home(base)
        };
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                let show_hidden = prefix.starts_with('.');
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if (!show_hidden && name.starts_with('.'))
                        || !starts_with_ignore_case(&name, prefix)
                        || !std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
                    {
                        continue;
                    }
                    matches.push(name);
                    if matches.len() >= MAX_MATCHES {
                        break;
                    }
                }
            }
            // Only worth reporting when the user typed a folder we cannot open.
            Err(_) => readable = base.is_empty(),
        }
        matches.sort_by_key(|m| m.to_lowercase());

        Self {
            base: base.to_string(),
            prefix: prefix.to_string(),
            matches,
            index: None,
            readable,
        }
    }

    pub fn matches(&self) -> &[String] {
        &self.matches
    }

    pub fn selected(&self) -> Option<usize> {
        self.index
    }

    pub fn readable(&self) -> bool {
        self.readable
    }

    /// Tab: finishes the folder name, or moves to the next suggestion.
    /// Returns the new input and whether this completion should be kept for more cycling.
    pub fn next(&mut self) -> Option<(String, bool)> {
        if self.matches.is_empty() {
            return None;
        }
        if let Some(i) = self.index {
            return Some((self.apply((i + 1) % self.matches.len()), true));
        }
        // First Tab: extend to what all suggestions share, if that adds anything.
        let common = common_prefix(&self.matches);
        if self.matches.len() > 1 && common.chars().count() > self.prefix.chars().count() {
            return Some((format!("{}{}", self.base, common), false));
        }
        Some((self.apply(0), true))
    }

    /// Up/Down and Shift+Tab: moves to the next or previous suggestion.
    pub fn step(&mut self, forward: bool) -> Option<String> {
        let n = self.matches.len();
        if n == 0 {
            return None;
        }
        let next = match (self.index, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
        };
        Some(self.apply(next))
    }

    fn apply(&mut self, i: usize) -> String {
        self.index = Some(i);
        let name = &self.matches[i];
        if name.ends_with(['/', '\\']) {
            format!("{}{name}", self.base)
        } else {
            format!("{}{name}{MAIN_SEPARATOR}", self.base)
        }
    }
}

fn starts_with_ignore_case(name: &str, prefix: &str) -> bool {
    name.to_lowercase().starts_with(&prefix.to_lowercase())
}

/// The longest start shared by all names, ignoring case, in the first name's spelling.
fn common_prefix(names: &[String]) -> String {
    let first: Vec<char> = names[0].chars().collect();
    let mut len = first.len();
    for name in &names[1..] {
        let shared = first
            .iter()
            .zip(name.chars())
            .take_while(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
            .count();
        len = len.min(shared);
    }
    first[..len].iter().collect()
}

/// Drive roots that exist on this machine, like `C:\`. Empty on other platforms.
fn drives() -> impl Iterator<Item = String> {
    let letters: &[char] = if cfg!(windows) {
        &[
            'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q',
            'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
        ]
    } else {
        &[]
    };
    letters
        .iter()
        .map(|c| format!("{c}:\\"))
        .filter(|d| std::path::Path::new(d).exists())
}

#[cfg(test)]
mod tests {
    use std::path::{MAIN_SEPARATOR, MAIN_SEPARATOR_STR, PathBuf};

    use super::{Completion, common_prefix};

    /// A throwaway folder tree: `Alpha`, `alpine`, `Beta`, and a plain file `alfile`.
    fn fixture(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("spotify-dl-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["Alpha", "alpine", "Beta"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("alfile"), "x").unwrap();
        root
    }

    fn typed(root: &std::path::Path, rest: &str) -> String {
        format!("{}{MAIN_SEPARATOR}{rest}", root.display())
    }

    #[test]
    fn lists_matching_folders_only_ignoring_case() {
        let root = fixture("list");
        let c = Completion::new(&typed(&root, "al"));
        assert_eq!(c.matches(), ["Alpha", "alpine"]);
        let all = Completion::new(&typed(&root, ""));
        assert_eq!(all.matches(), ["Alpha", "alpine", "Beta"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn first_tab_extends_to_shared_prefix_then_cycles() {
        let root = fixture("cycle");
        let mut c = Completion::new(&typed(&root, "a"));
        // "Alpha" and "alpine" share "Alp".
        let (text, keep) = c.next().unwrap();
        assert_eq!(text, typed(&root, "Alp"));
        assert!(!keep);

        let mut c = Completion::new(&text);
        let (first, keep) = c.next().unwrap();
        assert_eq!(first, typed(&root, "Alpha") + MAIN_SEPARATOR_STR);
        assert!(keep);
        let (second, _) = c.next().unwrap();
        assert_eq!(second, typed(&root, "alpine") + MAIN_SEPARATOR_STR);
        let (wrapped, _) = c.next().unwrap();
        assert_eq!(wrapped, first);
        let back = c.step(false).unwrap();
        assert_eq!(back, second);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn single_match_completes_with_separator() {
        let root = fixture("single");
        let mut c = Completion::new(&typed(&root, "be"));
        let (text, _) = c.next().unwrap();
        assert_eq!(text, typed(&root, "Beta") + MAIN_SEPARATOR_STR);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn no_matches_gives_nothing() {
        let root = fixture("none");
        let mut c = Completion::new(&typed(&root, "zzz"));
        assert!(c.matches().is_empty());
        assert!(c.next().is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn common_prefix_ignores_case() {
        assert_eq!(common_prefix(&["Alpha".into(), "alpine".into()]), "Alp");
        assert_eq!(common_prefix(&["x".into()]), "x");
    }
}
