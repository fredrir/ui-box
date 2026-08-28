use std::path::Path;

pub fn quote(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('\'');
    for ch in raw.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

pub fn quote_path(path: &Path) -> String {
    quote(&path.to_string_lossy())
}

pub fn slug(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

pub fn in_dir(dir: &Path, command: &str) -> String {
    format!("cd {} && {}", quote_path(dir), command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_plain_words() {
        assert_eq!(quote("cargo"), "'cargo'");
    }

    #[test]
    fn closes_and_reopens_around_a_single_quote() {
        assert_eq!(quote("it's"), r#"'it'\''s'"#);
    }

    #[test]
    fn keeps_spaces_and_metacharacters_inert() {
        assert_eq!(quote("a b; rm -rf /"), "'a b; rm -rf /'");
    }

    #[test]
    fn prefixes_a_directory_change() {
        assert_eq!(
            in_dir(Path::new("/home/fredrir/ArchTeX"), "cargo tauri build"),
            "cd '/home/fredrir/ArchTeX' && cargo tauri build"
        );
    }
}
