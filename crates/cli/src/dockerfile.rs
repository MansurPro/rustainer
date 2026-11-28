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
//!   `ONBUILD` in it does not do what it says here, and silently
//!   ignoring the line produces an image that is wrong in a way nobody
//!   will notice until production.

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    From {
        image: String,
        alias: Option<String>,
    },
    Run {
        /// Already resolved to an argv: shell form has been wrapped.
        argv: Vec<String>,
        /// `--mount=type=cache`: `(id, target)`, a directory that
        /// outlives the step and is not part of the layer.
        caches: Vec<(String, String)>,
        /// `--network=<name>`, for this step only.
        network: Option<String>,
        /// `--mount=type=secret`: `(id, target)`, a file the step can
        /// read and the image never holds.
        secrets: Vec<(String, String)>,
        /// `--mount=type=bind`: `(from, source, target)`, there for the
        /// step to read and not copied into the image. From the
        /// context unless `from` names a stage or an image.
        binds: Vec<(Option<String>, String, String)>,
        /// `--mount=type=ssh`: the caller's SSH agent, for this step.
        ssh: bool,
    },
    Copy {
        sources: Vec<String>,
        dest: String,
        /// `--from=<stage or image>`: read from there, not the context.
        from: Option<String>,
        /// `--chown=<user>[:<group>]`, names or numbers.
        chown: Option<String>,
        /// `--chmod=<octal>`.
        chmod: Option<String>,
        /// `ADD`, which unpacks a local tar archive where COPY copies it.
        unpack: bool,
    },
    /// The command that says whether the container is well, as an argv;
    /// empty for `HEALTHCHECK NONE`. Times in seconds.
    Healthcheck {
        test: Vec<String>,
        interval: u64,
        retries: u64,
    },
    StopSignal(String),
    /// An instruction for whoever builds FROM this image, kept as the
    /// text it was written as.
    OnBuild(String),
    Volume(Vec<String>),
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

/// `1m30s`, `500ms`, `45` - whole seconds, rounded up so that a
/// sub-second interval is not "never wait".
fn seconds(s: &str) -> Option<u64> {
    let (mut total_ms, mut digits) = (0u64, String::new());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let n: u64 = digits.parse().ok()?;
        digits.clear();
        total_ms += match c {
            'h' => n * 3_600_000,
            'm' if chars.peek() == Some(&'s') => {
                chars.next();
                n
            }
            'm' => n * 60_000,
            's' => n * 1000,
            _ => return None,
        };
    }
    if !digits.is_empty() {
        total_ms += digits.parse::<u64>().ok()? * 1000;
    }
    Some(total_ms.div_ceil(1000))
}

/// Split the `--flag=value` words off the front of an instruction.
fn leading_flags(rest: &str) -> (Vec<&str>, &str) {
    let mut flags = Vec::new();
    let mut rest = rest.trim_start();
    while rest.starts_with("--") {
        let (flag, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        flags.push(flag);
        rest = after.trim_start();
    }
    (flags, rest)
}

/// Every pair of an `ENV` or `LABEL`: `A=1 B="two words"` is two, and
/// the legacy `KEY rest of the line` is one.
fn key_values(rest: &str) -> Result<Vec<(String, String)>> {
    let first = rest.split_whitespace().next().unwrap_or("");
    if !first.contains('=') {
        return key_value(rest)
            .map(|kv| vec![kv])
            .ok_or_else(|| anyhow::anyhow!("needs KEY=value"));
    }
    crate::compose::words(rest)?
        .into_iter()
        .map(|w| {
            w.split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .ok_or_else(|| anyhow::anyhow!("{w:?} is not KEY=value"))
        })
        .collect()
}

pub fn parse(text: &str) -> Result<Vec<Instruction>> {
    let mut out = Vec::new();
    // What the shell form is wrapped in: `SHELL` changes it, for the
    // rest of the stage.
    let default_shell = || vec!["/bin/sh".to_owned(), "-c".to_owned()];
    let mut shell = default_shell();
    for (line_no, line) in logical_lines(text) {
        let (word, rest) = match line.split_once(char::is_whitespace) {
            Some((w, r)) => (w, r.trim()),
            None => (line.as_str(), ""),
        };
        let keyword = word.to_ascii_uppercase();
        let at = || format!("line {line_no}");
        let wrap = |rest: &str| -> Vec<String> {
            let mut v = shell.clone();
            v.push(rest.to_owned());
            v
        };

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
                shell = default_shell();
                Instruction::From { image, alias }
            }
            "RUN" => {
                let (flags, rest) = leading_flags(rest);
                let (mut caches, mut secrets, mut network) = (Vec::new(), Vec::new(), None);
                let mut binds = Vec::new();
                let mut ssh = false;
                for f in flags {
                    if let Some(n) = f.strip_prefix("--network=") {
                        network = Some(n.to_owned());
                    } else if let Some(m) = f.strip_prefix("--mount=") {
                        let opt = |keys: &[&str]| {
                            m.split(',')
                                .filter_map(|kv| kv.split_once('='))
                                .find(|(k, _)| keys.contains(k))
                                .map(|(_, v)| v.to_owned())
                        };
                        let target = opt(&["target", "dst", "destination"]);
                        match (opt(&["type"]).as_deref(), target) {
                            (Some("cache"), Some(t)) => {
                                caches.push((opt(&["id"]).unwrap_or_else(|| t.clone()), t));
                            }
                            (Some("secret"), target) => {
                                let id = opt(&["id"])
                                    .or_else(|| {
                                        let t = target.as_ref()?;
                                        Some(t.rsplit('/').next()?.to_owned())
                                    })
                                    .ok_or_else(|| {
                                        anyhow::anyhow!("{}: a secret mount needs an id", at())
                                    })?;
                                let target = target.unwrap_or_else(|| format!("/run/secrets/{id}"));
                                secrets.push((id, target));
                            }
                            // A bind with no type is what `--mount=target=`
                            // means. From the context only: one from a
                            // stage would need that stage mounted.
                            (Some("bind") | None, Some(t)) => {
                                let source = opt(&["source", "src"]).unwrap_or_else(|| ".".into());
                                binds.push((opt(&["from"]), source, t));
                            }
                            (Some("ssh"), _) => ssh = true,
                            (kind, _) => bail!(
                                "{}: RUN --mount of type {} is not implemented; \
                                 cache, secret, bind and ssh are",
                                at(),
                                kind.unwrap_or("bind")
                            ),
                        }
                    } else {
                        bail!("{}: {f} is not a RUN option this engine knows", at());
                    }
                }
                if rest.is_empty() {
                    bail!("{}: RUN needs a command", at());
                }
                // Shell form unless it is a list. The wrapping is the
                // whole difference: `&&`, globs and redirection only work
                // because a shell is there to do them.
                Instruction::Run {
                    argv: exec_form(rest)?.unwrap_or_else(|| wrap(rest)),
                    caches,
                    network,
                    secrets,
                    binds,
                    ssh,
                }
            }
            "COPY" | "ADD" => {
                let (flags, rest) = leading_flags(rest);
                let (mut from, mut chown, mut chmod) = (None, None, None);
                for f in flags {
                    if let Some(v) = f.strip_prefix("--from=") {
                        from = Some(v.to_owned());
                    } else if let Some(v) = f.strip_prefix("--chown=") {
                        chown = Some(v.to_owned());
                    } else if let Some(v) = f.strip_prefix("--chmod=") {
                        chmod = Some(v.to_owned());
                    } else if f != "--link" {
                        // --link only changes how BuildKit stores the
                        // layer, not what is in it.
                        bail!("{}: {f} is not a {keyword} option this engine knows", at());
                    }
                }
                let mut words = match exec_form(rest)? {
                    Some(list) => list,
                    None => rest.split_whitespace().map(str::to_owned).collect(),
                };
                if words.len() < 2 {
                    bail!(
                        "{}: {keyword} needs at least a source and a destination",
                        at()
                    );
                }
                let dest = words.pop().expect("checked above");
                Instruction::Copy {
                    sources: words,
                    dest,
                    from,
                    chown,
                    chmod,
                    unpack: keyword == "ADD",
                }
            }
            "ENV" | "LABEL" => {
                let pairs =
                    key_values(rest).map_err(|e| anyhow::anyhow!("{}: {keyword} {e}", at()))?;
                out.extend(pairs.into_iter().map(|(k, v)| match keyword.as_str() {
                    "ENV" => Instruction::Env(k, v),
                    _ => Instruction::Label(k, v),
                }));
                continue;
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
            "CMD" => Instruction::Cmd(exec_form(rest)?.unwrap_or_else(|| wrap(rest))),
            "ENTRYPOINT" => Instruction::Entrypoint(exec_form(rest)?.unwrap_or_else(|| wrap(rest))),
            "SHELL" => {
                shell = exec_form(rest)?.filter(|l| !l.is_empty()).ok_or_else(|| {
                    anyhow::anyhow!("{}: SHELL takes a list: [\"/bin/bash\", \"-c\"]", at())
                })?;
                continue;
            }
            "MAINTAINER" => Instruction::Label("maintainer".into(), rest.to_owned()),
            "STOPSIGNAL" => {
                if rest.is_empty() {
                    bail!("{}: STOPSIGNAL needs a signal", at());
                }
                Instruction::StopSignal(rest.to_owned())
            }
            "VOLUME" => Instruction::Volume(match exec_form(rest)? {
                Some(list) => list,
                None => rest.split_whitespace().map(str::to_owned).collect(),
            }),
            "HEALTHCHECK" => {
                let (flags, rest) = leading_flags(rest);
                let (mut interval, mut retries) = (30, 3);
                for f in flags {
                    let (k, v) = f.split_once('=').unwrap_or((f, ""));
                    let bad = || anyhow::anyhow!("{}: {f} is not a time or a count", at());
                    match k {
                        "--interval" => interval = seconds(v).ok_or_else(bad)?,
                        "--retries" => retries = v.parse().map_err(|_| bad())?,
                        // ponytail: a probe runs until it answers and a
                        // new container is `starting` until its retries
                        // run out, so these two have nothing to set.
                        "--timeout" | "--start-period" | "--start-interval" => {}
                        _ => bail!("{}: {f} is not a HEALTHCHECK option", at()),
                    }
                }
                let (word, cmd) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                let test = match word.to_ascii_uppercase().as_str() {
                    "NONE" => Vec::new(),
                    "CMD" if !cmd.trim().is_empty() => {
                        exec_form(cmd)?.unwrap_or_else(|| wrap(cmd.trim()))
                    }
                    _ => bail!("{}: HEALTHCHECK takes `CMD <command>` or `NONE`", at()),
                };
                Instruction::Healthcheck {
                    test,
                    interval,
                    retries,
                }
            }
            // Named rather than ignored. An image built from a Dockerfile
            // containing one of these does not do what the file says, and
            // skipping the line quietly is how that reaches production.
            "ONBUILD" => {
                // Read now, so a mistake in it is found by whoever wrote
                // it and not by whoever builds on the image later.
                let trigger = parse(&format!("FROM scratch\n{rest}"))
                    .map_err(|e| anyhow::anyhow!("{}: ONBUILD: {e}", at()))?;
                if trigger.len() != 2
                    || matches!(
                        trigger[1],
                        Instruction::OnBuild(_) | Instruction::From { .. }
                    )
                {
                    bail!(
                        "{}: ONBUILD takes one instruction, and not FROM or ONBUILD",
                        at()
                    );
                }
                Instruction::OnBuild(rest.to_owned())
            }
            "CROSS_BUILD" => {
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
            Instruction::Run {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "apk add curl && apk add jq".into()
                ],
                caches: vec![],
                network: None,
                secrets: vec![],
                binds: vec![],
                ssh: false,
            }
        );
    }

    #[test]
    fn shell_form_and_exec_form_are_different_instructions() {
        let d = parse("FROM a\nRUN echo hi && echo bye\nRUN [\"echo\", \"hi && bye\"]\n").unwrap();
        assert_eq!(
            d[1],
            Instruction::Run {
                argv: vec!["/bin/sh".into(), "-c".into(), "echo hi && echo bye".into()],
                caches: vec![],
                network: None,
                secrets: vec![],
                binds: vec![],
                ssh: false,
            }
        );
        // No shell, so `&&` is just two words inside one argument.
        assert_eq!(
            d[2],
            Instruction::Run {
                argv: vec!["echo".into(), "hi && bye".into()],
                caches: vec![],
                network: None,
                secrets: vec![],
                binds: vec![],
                ssh: false,
            }
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
                chown: None,
                chmod: None,
                unpack: false,
            }
        );
    }

    #[test]
    fn instructions_we_do_not_implement_are_refused_by_name() {
        // Not skipped. An image that quietly ignored HEALTHCHECK is not
        // the image the file describes.
        for line in ["CROSS_BUILD x", "RUN --mount=type=tmpfs,target=/x true"] {
            let e = parse(&format!("FROM a\n{line}\n")).unwrap_err().to_string();
            assert!(e.contains("not implemented"), "{line}: {e}");
        }
        let d = parse(
            "FROM a\nONBUILD COPY . /app\nRUN --mount=type=bind,source=req.txt,target=/r.txt true\n",
        )
        .unwrap();
        assert_eq!(d[1], Instruction::OnBuild("COPY . /app".into()));
        assert!(matches!(&d[2], Instruction::Run { binds, .. }
            if binds == &[(None, "req.txt".to_owned(), "/r.txt".to_owned())]));
        assert!(parse("FROM a\nONBUILD FROM b\n").is_err());
        assert!(parse("FROM a\nONBUILD FROB\n").is_err());
        let e = parse("FROM a\nFROB x\n").unwrap_err().to_string();
        assert!(e.contains("not a Dockerfile instruction"), "{e}");
    }

    #[test]
    fn the_rest_of_the_language() {
        let d = parse(
            "FROM a\n\
             SHELL [\"/bin/bash\", \"-c\"]\n\
             RUN --mount=type=cache,target=/root/.cache --network=none pip install x\n\
             COPY --chown=app:app --chmod=755 --link run.sh /usr/bin/\n\
             HEALTHCHECK --interval=1m30s --timeout=3s --retries=5 CMD curl -f localhost\n\
             HEALTHCHECK NONE\n\
             VOLUME /data /logs\n\
             STOPSIGNAL SIGQUIT\n\
             MAINTAINER someone\n\
             FROM b\n\
             RUN true\n",
        )
        .unwrap();
        assert_eq!(
            d[1],
            Instruction::Run {
                argv: vec!["/bin/bash".into(), "-c".into(), "pip install x".into()],
                caches: vec![("/root/.cache".into(), "/root/.cache".into())],
                network: Some("none".into()),
                secrets: vec![],
                binds: vec![],
                ssh: false,
            }
        );
        assert!(
            matches!(&d[2], Instruction::Copy { chown: Some(o), chmod: Some(m), unpack: false, .. } if o == "app:app" && m == "755")
        );
        assert_eq!(
            d[3],
            Instruction::Healthcheck {
                test: vec!["/bin/bash".into(), "-c".into(), "curl -f localhost".into()],
                interval: 90,
                retries: 5,
            }
        );
        assert!(matches!(&d[4], Instruction::Healthcheck { test, .. } if test.is_empty()));
        assert_eq!(
            d[5],
            Instruction::Volume(vec!["/data".into(), "/logs".into()])
        );
        assert_eq!(d[6], Instruction::StopSignal("SIGQUIT".into()));
        assert_eq!(
            d[7],
            Instruction::Label("maintainer".into(), "someone".into())
        );
        // SHELL lasts for its stage, not the file.
        assert!(matches!(&d[9], Instruction::Run { argv, .. } if argv[0] == "/bin/sh"));
        assert_eq!(seconds("500ms"), Some(1));
        assert_eq!(seconds("2h"), Some(7200));
        assert_eq!(seconds("soon"), None);
    }

    #[test]
    fn one_env_may_set_several_variables() {
        // The form most real files use, and one that used to give a
        // single variable whose value was the rest of the line.
        let d = parse(
            "FROM a\nENV A=1 \\\n    B=\"two words\" \\\n    PATH=/x:$PATH\n\
             LABEL org.x=y  z=w\n\
             RUN --mount=type=secret,id=npm,target=/root/.npmrc --mount=type=secret,id=tok npm ci\n",
        )
        .unwrap();
        assert_eq!(d[1], Instruction::Env("A".into(), "1".into()));
        assert_eq!(d[2], Instruction::Env("B".into(), "two words".into()));
        assert_eq!(d[3], Instruction::Env("PATH".into(), "/x:$PATH".into()));
        assert_eq!(d[5], Instruction::Label("z".into(), "w".into()));
        assert!(matches!(&d[6], Instruction::Run { secrets, .. }
            if secrets == &[("npm".to_owned(), "/root/.npmrc".to_owned()),
                            ("tok".to_owned(), "/run/secrets/tok".to_owned())]));
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
