//! Parsing a Dockerfile, and nothing else.
//!
//! Separate from the building so it can be tested without a kernel: every
//! shape below is a line somebody has written, and most of the bugs in a
//! Dockerfile reader are in deciding what a line *means* rather than in
//! running it.
//!
//! Three rules that look like details and are not:
//!
//! - **A backslash continues a line**, and the comment rules interact with
//!   it: a `#` line inside a continuation is still a comment and does not
//!   end the continuation. Getting that wrong silently truncates a `RUN`.
//! - **Every instruction has a shell form and an exec form.** `RUN apt
//!   update` goes through `/bin/sh -c`; `RUN ["apt", "update"]` does not,
//!   and so does no globbing, no `&&`, no variable expansion. They are
//!   different instructions that look alike.
//! - **Unknown instructions are refused, not skipped.** A Dockerfile with
//!   `HEALTHCHECK` in it does not do what it says here, and silently
//!   ignoring the line produces an image that is wrong in a way nobody
//!   will notice until production.

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    From {
        image: String,
        alias: Option<String>,
    },
    /// Already resolved to an argv: shell form has been wrapped.
    Run(Vec<String>),
    Copy {
        sources: Vec<String>,
        dest: String,
        /// `--from=<stage>`, which this engine does not implement.
        from: Option<String>,
    },
    Env(String, String),
    Workdir(String),
    Cmd(Vec<String>),
    Entrypoint(Vec<String>),
    User(String),
    Expose(String),
    Label(String, String),
    Arg(String, Option<String>),
}

/// One logical line: the instruction keyword and everything after it,
/// with continuations joined and comments removed.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut start_line = 0usize;
    let mut continuing = false;

    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();

        // A comment is a comment even in the middle of a continuation,
        // and crucially does NOT end it. `RUN a \` / `# why` / `  && b`
        // is one instruction, and a reader that stops at the comment
        // silently drops the `&& b`.
        if trimmed.starts_with('#') {
            continue;
        }
        if !continuing {
            if trimmed.is_empty() {
                continue;
            }
            start_line = n + 1;
            current.clear();
        }
        let (body, more) = match line.strip_suffix('\\') {
            Some(b) => (b, true),
            None => (line, false),
        };
        if continuing {
            current.push(' ');
            current.push_str(body.trim());
        } else {
            current.push_str(body.trim());
        }
        continuing = more;
        if !continuing && !current.trim().is_empty() {
            out.push((start_line, current.clone()));
            current.clear();
        }
    }
    if continuing && !current.trim().is_empty() {
        out.push((start_line, current));
    }
    out
}

/// `["a", "b"]` if it is the exec form, `None` if it is the shell form,
/// and an **error** if it was meant to be the exec form and is not valid
/// JSON.
///
/// Docker's own reader falls back to the shell form there, which turns a
/// misplaced quote into an instruction that runs something entirely
/// different and still succeeds. A line that starts with `[` was meant to
/// be a list.
fn exec_form(rest: &str) -> Result<Option<Vec<String>>> {
    let t = rest.trim();
    if !t.starts_with('[') {
        return Ok(None);
    }
    match serde_json::from_str::<Vec<String>>(t) {
        Ok(v) => Ok(Some(v)),
        Err(e) => bail!(
            "this looks like the exec form but is not valid JSON ({e}). The exec \
             form is a list of strings with double quotes: [\"/bin/sh\", \"-c\", \"...\"]. \
             Leave the brackets off for the shell form."
        ),
    }
}

/// `KEY=value` pairs, or the legacy `KEY value` form.
fn key_value(rest: &str) -> Option<(String, String)> {
    // Quotes around a value are part of the syntax, not the value, in
    // BOTH spellings. Stripping them in only one is how `ENV LANG
    // "C.UTF-8"` ends up with literal quotes inside the variable.
    let unquote = |v: &str| {
        let v = v.trim();
        v.strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(v)
            .to_owned()
    };
    let t = rest.trim();
    if let Some((k, v)) = t.split_once('=') {
        return Some((k.trim().to_owned(), unquote(v)));
    }
    let (k, v) = t.split_once(char::is_whitespace)?;
    Some((k.to_owned(), unquote(v)))
}

pub fn parse(text: &str) -> Result<Vec<Instruction>> {
    let mut out = Vec::new();
    for (line_no, line) in logical_lines(text) {
        let (word, rest) = match line.split_once(char::is_whitespace) {
            Some((w, r)) => (w, r.trim()),
            None => (line.as_str(), ""),
        };
        let keyword = word.to_ascii_uppercase();
        let at = || format!("line {line_no}");

        let instruction = match keyword.as_str() {
            "FROM" => {
                let mut parts = rest.split_whitespace();
                let image = parts
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("{}: FROM needs an image", at()))?
                    .to_owned();
                let alias = match (parts.next(), parts.next()) {
                    (Some(kw), Some(name)) if kw.eq_ignore_ascii_case("AS") => {
                        Some(name.to_owned())
                    }
                    (None, _) => None,
                    _ => bail!("{}: FROM takes an image, optionally `AS <name>`", at()),
                };
                Instruction::From { image, alias }
            }
            "RUN" => {
                if rest.is_empty() {
                    bail!("{}: RUN needs a command", at());
                }
                Instruction::Run(exec_form(rest)?.unwrap_or_else(|| {
                    // Shell form. The wrapping is the whole difference:
                    // `&&`, globs and redirection only work because a
                    // shell is there to do them.
                    vec!["/bin/sh".into(), "-c".into(), rest.to_owned()]
                }))
            }
            "COPY" | "ADD" => {
                let mut from = None;
                let mut words: Vec<String> = Vec::new();
                for w in rest.split_whitespace() {
                    if let Some(v) = w.strip_prefix("--from=") {
                        from = Some(v.to_owned());
                    } else if w.starts_with("--chown=") || w.starts_with("--chmod=") {
                        bail!(
                            "{}: {w} is not implemented. The file's ownership is \
                             whatever the build produced; set it with a RUN chown.",
                            at()
                        );
                    } else if w.starts_with("--") {
                        bail!("{}: {w} is not a {keyword} option this engine knows", at());
                    } else {
                        words.push(w.to_owned());
                    }
                }
                if let Some(list) = exec_form(rest)? {
                    words = list;
                }
                if words.len() < 2 {
                    bail!(
                        "{}: {keyword} needs at least a source and a destination",
                        at()
                    );
                }
                let dest = words.pop().expect("checked above");
                if keyword == "ADD" && words.iter().any(|w| w.starts_with("http")) {
                    bail!(
                        "{}: ADD from a URL is not implemented - it is a download \
                         with no checksum, which is why Docker's own documentation \
                         recommends RUN curl instead. Use that.",
                        at()
                    );
                }
                Instruction::Copy {
                    sources: words,
                    dest,
                    from,
                }
            }
            "ENV" => {
                let (k, v) = key_value(rest)
                    .ok_or_else(|| anyhow::anyhow!("{}: ENV needs KEY=value", at()))?;
                Instruction::Env(k, v)
            }
            "LABEL" => {
                let (k, v) = key_value(rest)
                    .ok_or_else(|| anyhow::anyhow!("{}: LABEL needs key=value", at()))?;
                Instruction::Label(k, v)
            }
            "ARG" => match rest.split_once('=') {
                Some((k, v)) => Instruction::Arg(k.trim().to_owned(), Some(v.trim().to_owned())),
                None if !rest.is_empty() => Instruction::Arg(rest.to_owned(), None),
                None => bail!("{}: ARG needs a name", at()),
            },
            "WORKDIR" => {
                if rest.is_empty() {
                    bail!("{}: WORKDIR needs a path", at());
                }
                Instruction::Workdir(rest.to_owned())
            }
            "USER" => {
                if rest.is_empty() {
                    bail!("{}: USER needs a name or id", at());
                }
                Instruction::User(rest.to_owned())
            }
            "EXPOSE" => Instruction::Expose(rest.to_owned()),
            "CMD" => Instruction::Cmd(
                exec_form(rest)?
                    .unwrap_or_else(|| vec!["/bin/sh".into(), "-c".into(), rest.to_owned()]),
            ),
            "ENTRYPOINT" => Instruction::Entrypoint(
                exec_form(rest)?
                    .unwrap_or_else(|| vec!["/bin/sh".into(), "-c".into(), rest.to_owned()]),
            ),
            // Named rather than ignored. An image built from a Dockerfile
            // containing one of these does not do what the file says, and
            // skipping the line quietly is how that reaches production.
            "HEALTHCHECK" | "ONBUILD" | "SHELL" | "STOPSIGNAL" | "VOLUME" | "MAINTAINER"
            | "CROSS_BUILD" => {
                bail!(
                    "{}: {keyword} is not implemented. It is skipped by no part of \
                     this engine - an image that silently ignored it would not be \
                     the image this file describes.",
                    at()
                )
            }
            other => bail!("{}: {other:?} is not a Dockerfile instruction", at()),
        };
        out.push(instruction);
    }

    match out.first() {
        Some(Instruction::From { .. }) => {}
        Some(Instruction::Arg(..)) => {}
        _ => bail!("a Dockerfile must begin with FROM"),
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_continuation_survives_a_comment_in_the_middle_of_it() {
        // The rule that silently truncates a RUN when it is missed.
        let d = parse(
            "FROM alpine\n\
             RUN apk add curl \\\n\
             # why we also need jq\n\
             && apk add jq\n",
        )
        .unwrap();
        assert_eq!(
            d[1],
            Instruction::Run(vec![
                "/bin/sh".into(),
                "-c".into(),
                "apk add curl && apk add jq".into()
            ])
        );
    }

    #[test]
    fn shell_form_and_exec_form_are_different_instructions() {
        let d = parse("FROM a\nRUN echo hi && echo bye\nRUN [\"echo\", \"hi && bye\"]\n").unwrap();
        assert_eq!(
            d[1],
            Instruction::Run(vec![
                "/bin/sh".into(),
                "-c".into(),
                "echo hi && echo bye".into()
            ])
        );
        // No shell, so `&&` is just two words inside one argument.
        assert_eq!(
            d[2],
            Instruction::Run(vec!["echo".into(), "hi && bye".into()])
        );
    }

    #[test]
    fn env_takes_both_spellings_and_strips_the_quotes() {
        let d = parse("FROM a\nENV TZ=UTC\nENV LANG \"C.UTF-8\"\n").unwrap();
        assert_eq!(d[1], Instruction::Env("TZ".into(), "UTC".into()));
        assert_eq!(d[2], Instruction::Env("LANG".into(), "C.UTF-8".into()));
    }

    #[test]
    fn copy_takes_several_sources_and_the_last_word_is_the_destination() {
        let d = parse("FROM a\nCOPY a.txt b.txt /dst/\n").unwrap();
        assert_eq!(
            d[1],
            Instruction::Copy {
                sources: vec!["a.txt".into(), "b.txt".into()],
                dest: "/dst/".into(),
                from: None,
            }
        );
    }

    #[test]
    fn instructions_we_do_not_implement_are_refused_by_name() {
        // Not skipped. An image that quietly ignored HEALTHCHECK is not
        // the image the file describes.
        for line in ["HEALTHCHECK CMD true", "ONBUILD RUN x", "VOLUME /data"] {
            let e = parse(&format!("FROM a\n{line}\n")).unwrap_err().to_string();
            assert!(e.contains("not implemented"), "{line}: {e}");
        }
        let e = parse("FROM a\nADD https://x/y /y\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("no checksum"), "{e}");
        let e = parse("FROM a\nFROB x\n").unwrap_err().to_string();
        assert!(e.contains("not a Dockerfile instruction"), "{e}");
    }

    #[test]
    fn a_bracketed_line_that_is_not_json_is_an_error() {
        // Docker falls back to the shell form here, which turns a
        // misplaced quote into an instruction that runs something else
        // entirely and still succeeds.
        let e = parse("FROM a\nCMD [\"sh\", \"-c\", \"echo \\$X\"]\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("exec form"), "{e}");
    }

    #[test]
    fn a_file_that_does_not_start_with_from_is_refused() {
        assert!(parse("RUN true\n").is_err());
        // ARG before FROM is legal and common, for the base image's tag.
        assert!(parse("ARG V=3.21\nFROM alpine:${V}\n").is_ok());
    }

    #[test]
    fn errors_name_the_line() {
        let e = parse("FROM a\n\n\nRUN\n").unwrap_err().to_string();
        assert!(e.contains("line 4"), "{e}");
    }

    #[test]
    fn a_trailing_continuation_is_not_lost() {
        // `RUN x \` as the last line is malformed, but dropping it
        // silently is worse than building it.
        let d = parse("FROM a\nRUN echo hi \\\n").unwrap();
        assert_eq!(d.len(), 2);
    }
}
