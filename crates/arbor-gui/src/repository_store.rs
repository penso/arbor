use {
    arbor_core::worktree,
    std::{collections::HashSet, env, fs, path::PathBuf},
};

const REPOSITORY_STORE_RELATIVE_PATH: &str = ".arbor/repositories.json";

pub trait RepositoryStore {
    fn load_roots(&self) -> Result<Vec<PathBuf>, String>;
    fn save_roots(&self, roots: &[PathBuf]) -> Result<(), String>;
}

pub struct JsonRepositoryStore {
    path: PathBuf,
}

impl JsonRepositoryStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl RepositoryStore for JsonRepositoryStore {
    fn load_roots(&self) -> Result<Vec<PathBuf>, String> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let raw = fs::read_to_string(&self.path).map_err(|error| {
            format!(
                "failed to read repository store `{}`: {error}",
                self.path.display()
            )
        })?;
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }

        parse_json_string_array(&raw).map(|roots| roots.into_iter().map(PathBuf::from).collect())
    }

    fn save_roots(&self, roots: &[PathBuf]) -> Result<(), String> {
        let serialized_roots: Vec<String> = roots
            .iter()
            .map(|root| root.display().to_string())
            .collect();
        let content = serialize_json_string_array(&serialized_roots);

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create repository store directory `{}`: {error}",
                    parent.display()
                )
            })?;
        }

        fs::write(&self.path, content).map_err(|error| {
            format!(
                "failed to write repository store `{}`: {error}",
                self.path.display()
            )
        })
    }
}

pub fn default_repository_store() -> Box<dyn RepositoryStore> {
    Box::new(JsonRepositoryStore::new(default_repository_store_path()))
}

fn default_repository_store_path() -> PathBuf {
    match env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(REPOSITORY_STORE_RELATIVE_PATH),
        Err(_) => PathBuf::from(REPOSITORY_STORE_RELATIVE_PATH),
    }
}

pub fn resolve_repositories_from_roots(roots: Vec<PathBuf>) -> Vec<crate::RepositorySummary> {
    let mut repositories = Vec::new();
    let mut seen_roots = HashSet::new();

    for root in roots {
        if root.as_os_str().is_empty() {
            continue;
        }

        let resolved_root = canonicalize_if_possible(root);
        let resolved_root = match worktree::repo_root(&resolved_root) {
            Ok(path) => canonicalize_if_possible(path),
            Err(_) => continue,
        };

        if seen_roots.insert(resolved_root.clone()) {
            repositories.push(crate::RepositorySummary::from_root(resolved_root));
        }
    }

    repositories
}

pub fn repository_roots_from_summaries(repositories: &[crate::RepositorySummary]) -> Vec<PathBuf> {
    repositories
        .iter()
        .map(|repository| repository.root.clone())
        .collect()
}

fn canonicalize_if_possible(path: PathBuf) -> PathBuf {
    worktree::canonicalize_if_possible(path)
}

fn serialize_json_string_array(values: &[String]) -> String {
    let mut output = String::from("[");
    if values.is_empty() {
        output.push_str("]\n");
        return output;
    }

    output.push('\n');
    for (index, value) in values.iter().enumerate() {
        output.push_str("  \"");
        output.push_str(&escape_json_string(value));
        output.push('"');
        if index + 1 != values.len() {
            output.push(',');
        }
        output.push('\n');
    }
    output.push_str("]\n");
    output
}

fn escape_json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            ch if ch.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", ch as u32));
            },
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn parse_json_string_array(input: &str) -> Result<Vec<String>, String> {
    let mut index = 0;
    skip_json_whitespace(input, &mut index);
    if !consume_json_char(input, &mut index, '[') {
        return Err("repository store JSON must start with `[`".to_owned());
    }

    let mut values = Vec::new();
    loop {
        skip_json_whitespace(input, &mut index);
        if consume_json_char(input, &mut index, ']') {
            break;
        }

        values.push(parse_json_string(input, &mut index)?);
        skip_json_whitespace(input, &mut index);

        if consume_json_char(input, &mut index, ',') {
            continue;
        }
        if consume_json_char(input, &mut index, ']') {
            break;
        }
        return Err("expected `,` or `]` in repository store JSON array".to_owned());
    }

    skip_json_whitespace(input, &mut index);
    if index != input.len() {
        return Err("unexpected trailing characters in repository store JSON".to_owned());
    }

    Ok(values)
}

fn skip_json_whitespace(input: &str, index: &mut usize) {
    while let Some(ch) = peek_json_char(input, *index) {
        if !ch.is_whitespace() {
            break;
        }
        *index += ch.len_utf8();
    }
}

fn consume_json_char(input: &str, index: &mut usize, expected: char) -> bool {
    let Some(ch) = peek_json_char(input, *index) else {
        return false;
    };
    if ch != expected {
        return false;
    }
    *index += ch.len_utf8();
    true
}

fn peek_json_char(input: &str, index: usize) -> Option<char> {
    input.get(index..)?.chars().next()
}

fn next_json_char(input: &str, index: &mut usize) -> Option<char> {
    let ch = peek_json_char(input, *index)?;
    *index += ch.len_utf8();
    Some(ch)
}

fn parse_json_string(input: &str, index: &mut usize) -> Result<String, String> {
    if !consume_json_char(input, index, '"') {
        return Err("expected string value in repository store JSON".to_owned());
    }

    let mut value = String::new();
    loop {
        let ch = next_json_char(input, index)
            .ok_or_else(|| "unterminated string in repository store JSON".to_owned())?;
        match ch {
            '"' => return Ok(value),
            '\\' => {
                let escape = next_json_char(input, index)
                    .ok_or_else(|| "unterminated escape in repository store JSON".to_owned())?;
                match escape {
                    '"' => value.push('"'),
                    '\\' => value.push('\\'),
                    '/' => value.push('/'),
                    'b' => value.push('\u{08}'),
                    'f' => value.push('\u{0c}'),
                    'n' => value.push('\n'),
                    'r' => value.push('\r'),
                    't' => value.push('\t'),
                    'u' => value.push(parse_json_unicode_escape(input, index)?),
                    _ => {
                        return Err("invalid escape in repository store JSON".to_owned());
                    },
                }
            },
            _ if ch.is_control() => {
                return Err("control character in repository store JSON string".to_owned());
            },
            _ => value.push(ch),
        }
    }
}

fn parse_json_unicode_escape(input: &str, index: &mut usize) -> Result<char, String> {
    let mut codepoint = 0u32;
    for _ in 0..4 {
        let ch = next_json_char(input, index)
            .ok_or_else(|| "unterminated unicode escape in repository store JSON".to_owned())?;
        let digit = ch
            .to_digit(16)
            .ok_or_else(|| "invalid unicode escape in repository store JSON".to_owned())?;
        codepoint = (codepoint << 4) | digit;
    }
    char::from_u32(codepoint)
        .ok_or_else(|| "invalid unicode codepoint in repository store JSON".to_owned())
}
