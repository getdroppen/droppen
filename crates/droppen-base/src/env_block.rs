//! A block of `KEY=VALUE` lines, as a person types an environment for a CLI:
//! an env-mode account's credential, and an ACP agent's own variables. Read
//! with [`parse_env_block`], written back with [`EnvVar::render`].
//!
//! A name in backticks such as `AccountsStore` in these docs is an item of
//! the server, which this crate cannot link to.

use serde::{Deserialize, Serialize};

/// Why a block was refused: one sentence for the person who typed it, which
/// never quotes the text it refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidEnvBlock(
    /// The sentence.
    pub String,
);

impl std::fmt::Display for InvalidEnvBlock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InvalidEnvBlock {}

/// One `KEY=VALUE` pair of an env-mode account, in the order it was typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    /// The variable's name. Shown back to the user; never a secret.
    pub key: String,
    /// Its value. Part of no list response and never logged; read back only by
    /// the explicit `AccountsStore::env_block` route the editor calls.
    pub value: String,
}

/// Variables a block may not set, because this server sets them.
///
/// Not a security boundary — the user already chooses what the CLI runs — but
/// the difference between "my token did not work" and "remote-hand cannot find
/// my sessions any more". `CLAUDE_CONFIG_DIR` and `CODEX_HOME` decide where a
/// transcript lands, which is the whole of how a session is discovered and
/// resumed; `HOME` and `PATH` move everything at once.
pub const DENIED_ENV_KEYS: [&str; 10] = [
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "HOME",
    "PATH",
    "NODE_OPTIONS",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
];
/// At most this many variables in one block.
const MAX_ENV_VARS: usize = 64;
/// At most this much text in one block.
const MAX_ENV_BYTES: usize = 8 * 1024;

/// Read a `KEY=VALUE`-per-line block the way a `.env` file is read.
///
/// Deliberately *not* a dotenv implementation. No interpolation, no escapes, no
/// multi-line values: this text is going straight into a child's environment,
/// and every feature that rewrites a value is a way for the thing that ends up
/// there to differ from the thing that was typed. Quotes are stripped because
/// people paste them out of shell snippets, and that is the whole of it.
///
/// Refuses rather than skips, everywhere it can: a line that is not a comment,
/// not blank and not a pair is a typo, and silently dropping it would store a
/// block that is missing the variable the user thinks they set.
pub fn parse_env_block(text: &str) -> Result<Vec<EnvVar>, InvalidEnvBlock> {
    // A block pasted out of a Windows editor may open with a BOM; the browser's
    // `trim()` drops it, ours must too or line 1 is refused for a byte nobody sees.
    let text = text.trim_start_matches('\u{feff}');
    if text.len() > MAX_ENV_BYTES {
        return Err(InvalidEnvBlock(format!(
            "the environment block is larger than {} KiB",
            MAX_ENV_BYTES / 1024
        )));
    }
    let mut parsed: Vec<EnvVar> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `export FOO=bar` is what a shell snippet looks like, and pasting one
        // in is the most likely way this box gets filled.
        let line = line.strip_prefix("export ").map(str::trim_start).unwrap_or(line);
        // Neither message quotes the text it refused. A `400` body is read by
        // the browser, by every log in front of it and by whatever the client
        // does with an error string — and the likeliest thing on a line that is
        // not a pair is the credential itself, pasted before its name was typed.
        let Some((key, value)) = line.split_once('=') else {
            return Err(InvalidEnvBlock(format!("line {number} is not a KEY=VALUE pair")));
        };
        let key = key.trim();
        if !is_env_name(key) {
            return Err(InvalidEnvBlock(format!("line {number} does not start with a variable name")));
        }
        if DENIED_ENV_KEYS.contains(&key) {
            return Err(InvalidEnvBlock(format!(
                "{key} is set by Droppen itself; setting it here would move where this session's transcript lives"
            )));
        }
        if parsed.iter().any(|var| var.key == key) {
            return Err(InvalidEnvBlock(format!("{key} is set twice; keep one of them")));
        }
        if parsed.len() == MAX_ENV_VARS {
            return Err(InvalidEnvBlock(format!("more than {MAX_ENV_VARS} variables in one block")));
        }
        parsed.push(EnvVar {
            key: key.to_string(),
            value: unquote(value.trim()).to_string(),
        });
    }
    if parsed.is_empty() {
        return Err(InvalidEnvBlock("the environment block is empty".to_string()));
    }
    Ok(parsed)
}

impl EnvVar {
    /// This variable as a line [`parse_env_block`] reads back unchanged.
    ///
    /// A value with no surrounding whitespace needs no quoting. One that has it
    /// does: the parser trims a line's value, so an unquoted ` x ` would come back
    /// as `x`. Quoting only applies when both ends agree because that is the only
    /// shape [`unquote`] strips.
    pub fn render(&self) -> String {
        if self.value.trim() != self.value {
            format!("{}=\"{}\"", self.key, self.value)
        } else {
            format!("{}={}", self.key, self.value)
        }
    }
}

/// `"x"` and `'x'` become `x`; anything else is returned as it stands.
///
/// No escape processing inside, on purpose: a backslash in a pasted token is a
/// backslash, and "helpfully" unescaping it would hand the CLI a value nobody
/// typed.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return &value[1..value.len() - 1];
        }
    }
    value
}

/// `^[A-Za-z_][A-Za-z0-9_]*$`, spelled out.
pub fn is_env_name(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_alphabetic() || first == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole of the box's grammar, in one place.
    #[test]
    fn a_block_is_read_the_way_a_person_pastes_one() {
        let parsed = parse_env_block(
            "# a comment\n\
             \n\
             ANTHROPIC_API_KEY=sk-ant-plain\n\
             export ANTHROPIC_BASE_URL=https://gw.example.com/v1\n\
               ANTHROPIC_AUTH_TOKEN = \"quoted value\"  \n\
             SINGLE='single quoted'\n\
             EMPTY=\n\
             WITH_EQUALS=a=b=c\n",
        )
        .expect("a block a person would actually type");
        assert_eq!(
            parsed,
            vec![
                EnvVar {
                    key: "ANTHROPIC_API_KEY".into(),
                    value: "sk-ant-plain".into()
                },
                EnvVar {
                    key: "ANTHROPIC_BASE_URL".into(),
                    value: "https://gw.example.com/v1".into()
                },
                EnvVar {
                    key: "ANTHROPIC_AUTH_TOKEN".into(),
                    value: "quoted value".into()
                },
                EnvVar {
                    key: "SINGLE".into(),
                    value: "single quoted".into()
                },
                EnvVar {
                    key: "EMPTY".into(),
                    value: String::new()
                },
                EnvVar {
                    key: "WITH_EQUALS".into(),
                    value: "a=b=c".into()
                },
            ],
            "order is kept, `export` and quotes come off, the first `=` splits"
        );
        // No escape processing: a backslash in a token is a backslash.
        assert_eq!(parse_env_block("K=a\\nb").expect("literal")[0].value, "a\\nb");
        assert_eq!(parse_env_block("_UNDERSCORE1=x").expect("a legal name")[0].key, "_UNDERSCORE1");
    }

    /// A line that is not a pair is a typo, and dropping it silently would
    /// store a block that is missing the variable the user thinks they set.
    #[test]
    fn a_block_refuses_rather_than_skips_what_it_cannot_read() {
        let refused = |block: &str| match parse_env_block(block) {
            Err(InvalidEnvBlock(message)) => message,
            other => panic!("expected a refusal for {block:?}, got {other:?}"),
        };

        assert!(refused("A=1\njust some words\n").contains("line 2"), "the refusal names the line");
        assert!(refused("A=1\n1BAD=2").contains("line 2"));
        assert!(refused("no-equals-at-all").contains("line 1"));
        // And never the text it refused: the likeliest thing on a line that is
        // not a pair is the credential, pasted before its name was typed, and
        // this message is a `400` body.
        let pasted = refused("ANTHROPIC_AUTH_TOKEN sk-ant-oat01-PastedBeforeItsName");
        assert!(!pasted.contains("sk-ant"), "a refusal must not quote the line: {pasted}");
        let odd = refused("sk-ant-oat01-PastedBeforeItsName=");
        assert!(!odd.contains("sk-ant"), "nor the half of it before the `=`: {odd}");
        assert!(refused("A=1\nA=2").contains("A is set twice"));
        assert!(refused("").contains("empty"), "an empty box is not a credential");
        assert!(refused("# only a comment\n\n").contains("empty"));

        // The names and the limits are written out rather than read back from
        // the constants, so a dropped name or a raised limit fails here.
        for denied in [
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
            "HOME",
            "PATH",
            "NODE_OPTIONS",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
            "DYLD_FRAMEWORK_PATH",
        ] {
            let message = refused(&format!("OPENAI_API_KEY=k\n{denied}=/somewhere"));
            assert!(message.contains(denied), "the refusal names the key: {message}");
        }
        assert_eq!(DENIED_ENV_KEYS.len(), 10, "and no other name is refused");
        // A name matches as written: `ld_preload` is not the loader's variable.
        assert_eq!(parse_env_block("ld_preload=x").expect("another name")[0].key, "ld_preload");

        let vars = |n: usize| (0..n).map(|i| format!("K{i}=v")).collect::<Vec<_>>().join("\n");
        assert_eq!(parse_env_block(&vars(64)).expect("64 variables").len(), 64);
        assert!(refused(&vars(65)).contains("more than 64"));
        let sized = |n: usize| format!("K=\"{}\"", "x".repeat(n - 4));
        assert_eq!(sized(8192).len(), 8192);
        assert!(parse_env_block(&sized(8192)).is_ok(), "8192 bytes is the limit, not past it");
        assert!(refused(&sized(8193)).contains("8 KiB"));
    }
}
