//! Pure CLI flag parsing (Phase 8, step 8.1 of `docs/IMPLEMENTATION-PLAN.md`).
//!
//! `parse` never touches the filesystem, stdin, or `std::env` — it takes the
//! already-collected argument list and returns a decision, so the whole
//! surface is covered by ordinary unit tests. Wiring the result up to actual
//! rendering/file I/O is Phase 8.2's job (`main.rs`).

/// The five theme names `mud`/`mudl` ship (see Appendix B/C of the
/// implementation plan) — the only values `--theme` accepts.
pub const VALID_THEMES: &[&str] = &["austere", "blues", "earthy", "riot", "system"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderArgs {
    pub mode: Mode,
    pub files: Vec<String>,
    pub standalone: bool,
    pub fragment: bool,
    pub line_numbers: bool,
    pub word_wrap: bool,
    pub readable_column: bool,
    pub theme: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedArgs {
    Help,
    Version,
    InstallCli,
    Render(RenderArgs),
    LaunchGui(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgError(pub String);

impl std::fmt::Display for ArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Parses an already-collected argument list (i.e. `std::env::args()` with
/// argv[0] already stripped).
pub fn parse(args: &[String]) -> Result<ParsedArgs, ArgError> {
    let mut mode: Option<Mode> = None;
    let mut files = Vec::new();
    let mut standalone = false;
    let mut fragment = false;
    let mut line_numbers = false;
    let mut word_wrap = false;
    let mut readable_column = false;
    let mut theme: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--help" | "-h" => return Ok(ParsedArgs::Help),
            "--version" | "-v" => return Ok(ParsedArgs::Version),
            "--install-cli" => return Ok(ParsedArgs::InstallCli),
            "--html-up" | "-u" => {
                if mode == Some(Mode::Down) {
                    return Err(ArgError(
                        "--html-up and --html-down are mutually exclusive".to_string(),
                    ));
                }
                mode = Some(Mode::Up);
            }
            "--html-down" | "-d" => {
                if mode == Some(Mode::Up) {
                    return Err(ArgError(
                        "--html-up and --html-down are mutually exclusive".to_string(),
                    ));
                }
                mode = Some(Mode::Down);
            }
            "--standalone" => standalone = true,
            "--fragment" | "-f" => fragment = true,
            "--line-numbers" => line_numbers = true,
            "--word-wrap" => word_wrap = true,
            "--readable-column" => readable_column = true,
            _ if arg == "--theme" || arg.starts_with("--theme=") => {
                let name = if let Some(name) = arg.strip_prefix("--theme=") {
                    name.to_string()
                } else {
                    i += 1;
                    match args.get(i) {
                        Some(name) => name.clone(),
                        None => return Err(ArgError("--theme requires a value".to_string())),
                    }
                };
                if !VALID_THEMES.contains(&name.as_str()) {
                    return Err(ArgError(format!(
                        "invalid theme '{name}': expected one of {}",
                        VALID_THEMES.join(", ")
                    )));
                }
                theme = Some(name);
            }
            _ if arg.starts_with('-') && arg != "-" => {
                return Err(ArgError(format!("unknown flag: {arg}")));
            }
            _ => files.push(arg.clone()),
        }
        i += 1;
    }

    let Some(mode) = mode else {
        return Ok(ParsedArgs::LaunchGui(files));
    };

    Ok(ParsedArgs::Render(RenderArgs {
        mode,
        files,
        standalone,
        fragment,
        line_numbers,
        word_wrap,
        readable_column,
        theme,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn help_long() {
        assert_eq!(parse(&args(&["--help"])), Ok(ParsedArgs::Help));
    }

    #[test]
    fn help_short() {
        assert_eq!(parse(&args(&["-h"])), Ok(ParsedArgs::Help));
    }

    #[test]
    fn version_long() {
        assert_eq!(parse(&args(&["--version"])), Ok(ParsedArgs::Version));
    }

    #[test]
    fn version_short() {
        assert_eq!(parse(&args(&["-v"])), Ok(ParsedArgs::Version));
    }

    #[test]
    fn install_cli_flag() {
        assert_eq!(parse(&args(&["--install-cli"])), Ok(ParsedArgs::InstallCli));
    }

    #[test]
    fn no_files_no_flags_launches_gui_with_empty_list() {
        assert_eq!(parse(&args(&[])), Ok(ParsedArgs::LaunchGui(vec![])));
    }

    #[test]
    fn files_with_no_render_flag_launches_gui_with_files() {
        assert_eq!(
            parse(&args(&["a.md", "b.md"])),
            Ok(ParsedArgs::LaunchGui(vec![
                "a.md".to_string(),
                "b.md".to_string()
            ]))
        );
    }

    #[test]
    fn html_up_short_renders_up() {
        let parsed = parse(&args(&["-u", "a.md"])).unwrap();
        match parsed {
            ParsedArgs::Render(r) => {
                assert_eq!(r.mode, Mode::Up);
                assert_eq!(r.files, vec!["a.md".to_string()]);
            }
            other => panic!("expected Render, got {other:?}"),
        }
    }

    #[test]
    fn html_up_long_renders_up() {
        let parsed = parse(&args(&["--html-up", "a.md"])).unwrap();
        match parsed {
            ParsedArgs::Render(r) => assert_eq!(r.mode, Mode::Up),
            other => panic!("expected Render, got {other:?}"),
        }
    }

    #[test]
    fn html_down_short_renders_down() {
        let parsed = parse(&args(&["-d", "a.md"])).unwrap();
        match parsed {
            ParsedArgs::Render(r) => assert_eq!(r.mode, Mode::Down),
            other => panic!("expected Render, got {other:?}"),
        }
    }

    #[test]
    fn html_down_long_renders_down() {
        let parsed = parse(&args(&["--html-down", "a.md"])).unwrap();
        match parsed {
            ParsedArgs::Render(r) => assert_eq!(r.mode, Mode::Down),
            other => panic!("expected Render, got {other:?}"),
        }
    }

    #[test]
    fn up_and_down_together_is_an_error() {
        let err = parse(&args(&["-u", "-d", "a.md"])).unwrap_err();
        assert!(err.0.contains("mutually exclusive"));
    }

    #[test]
    fn down_and_up_together_is_an_error() {
        let err = parse(&args(&["-d", "-u", "a.md"])).unwrap_err();
        assert!(err.0.contains("mutually exclusive"));
    }

    #[test]
    fn repeated_same_mode_flag_is_not_an_error() {
        let parsed = parse(&args(&["-u", "-u", "a.md"])).unwrap();
        match parsed {
            ParsedArgs::Render(r) => assert_eq!(r.mode, Mode::Up),
            other => panic!("expected Render, got {other:?}"),
        }
    }

    #[test]
    fn standalone_flag_sets_standalone() {
        let parsed = parse(&args(&["-u", "--standalone", "a.md"])).unwrap();
        match parsed {
            ParsedArgs::Render(r) => assert!(r.standalone),
            other => panic!("expected Render, got {other:?}"),
        }
    }

    #[test]
    fn fragment_short_and_long() {
        for flag in ["--fragment", "-f"] {
            let parsed = parse(&args(&["-u", flag, "a.md"])).unwrap();
            match parsed {
                ParsedArgs::Render(r) => assert!(r.fragment),
                other => panic!("expected Render, got {other:?}"),
            }
        }
    }

    #[test]
    fn line_numbers_word_wrap_readable_column_flags() {
        let parsed = parse(&args(&[
            "-u",
            "--line-numbers",
            "--word-wrap",
            "--readable-column",
            "a.md",
        ]))
        .unwrap();
        match parsed {
            ParsedArgs::Render(r) => {
                assert!(r.line_numbers);
                assert!(r.word_wrap);
                assert!(r.readable_column);
            }
            other => panic!("expected Render, got {other:?}"),
        }
    }

    #[test]
    fn theme_with_space_separated_value() {
        let parsed = parse(&args(&["-u", "--theme", "blues", "a.md"])).unwrap();
        match parsed {
            ParsedArgs::Render(r) => assert_eq!(r.theme, Some("blues".to_string())),
            other => panic!("expected Render, got {other:?}"),
        }
    }

    #[test]
    fn theme_with_equals_value() {
        let parsed = parse(&args(&["-u", "--theme=riot", "a.md"])).unwrap();
        match parsed {
            ParsedArgs::Render(r) => assert_eq!(r.theme, Some("riot".to_string())),
            other => panic!("expected Render, got {other:?}"),
        }
    }

    #[test]
    fn theme_invalid_name_is_an_error_naming_valid_set() {
        let err = parse(&args(&["-u", "--theme", "nonsense", "a.md"])).unwrap_err();
        assert!(err.0.contains("nonsense"));
        for valid in VALID_THEMES {
            assert!(err.0.contains(valid));
        }
    }

    #[test]
    fn theme_missing_value_is_an_error() {
        assert!(parse(&args(&["-u", "--theme"])).is_err());
    }

    #[test]
    fn unknown_flag_is_an_error() {
        let err = parse(&args(&["--nonsense"])).unwrap_err();
        assert!(err.0.contains("--nonsense"));
    }

    #[test]
    fn unknown_short_flag_is_an_error() {
        assert!(parse(&args(&["-z"])).is_err());
    }

    #[test]
    fn multiple_files_with_up_are_all_collected() {
        let parsed = parse(&args(&["-u", "a.md", "b.md", "c.md"])).unwrap();
        match parsed {
            ParsedArgs::Render(r) => assert_eq!(
                r.files,
                vec!["a.md".to_string(), "b.md".to_string(), "c.md".to_string()]
            ),
            other => panic!("expected Render, got {other:?}"),
        }
    }

    #[test]
    fn fragment_combined_with_noop_flags_still_parses() {
        let parsed = parse(&args(&[
            "-u",
            "--fragment",
            "--line-numbers",
            "--word-wrap",
            "a.md",
        ]))
        .unwrap();
        match parsed {
            ParsedArgs::Render(r) => {
                assert!(r.fragment);
                assert!(r.line_numbers);
                assert!(r.word_wrap);
            }
            other => panic!("expected Render, got {other:?}"),
        }
    }

    #[test]
    fn bare_dash_is_treated_as_a_file_argument_not_a_flag() {
        let parsed = parse(&args(&["-u", "-"])).unwrap();
        match parsed {
            ParsedArgs::Render(r) => assert_eq!(r.files, vec!["-".to_string()]),
            other => panic!("expected Render, got {other:?}"),
        }
    }
}
