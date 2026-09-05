mod args;
mod installer;

use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode, Stdio};

use args::{parse, ArgError, Mode, ParsedArgs, RenderArgs};
use installer::RealFileSystem as InstallerFileSystem;
use mudl_core::images::rewrite_srcs_to_data_uris;
use mudl_core::options::RenderOptions;
use mudl_core::render::{render_down, render_up};
use mudl_core::resources;
use mudl_core::template::{select_assets, HtmlDocument, Script};

const HELP_TEXT: &str = "\
mudl - A Perfect Markdown Viewer

USAGE:
    mudl [FLAGS] [FILE...]

FLAGS:
    -h, --help              Print this help message and exit
    -v, --version           Print the version and exit
    -u, --html-up           Render Up-mode (styled) HTML to stdout
    -d, --html-down         Render Down-mode (raw source) HTML to stdout
        --standalone        Inline local images as data URIs (implied by the
                             default full-document output; only meaningful
                             together with --fragment)
    -f, --fragment          Emit body-only HTML, no document wrapper
                             (default: a complete, self-contained document)
        --line-numbers      Show line numbers (Down mode)
        --word-wrap         Wrap long lines (Down mode)
        --readable-column   Constrain body width to a readable column
        --theme NAME        One of: austere, blues, earthy, riot, system
        --install-cli       Symlink this binary into ~/.local/bin/mudl

With no FILE arguments, reads Markdown from stdin. With no render flag
(-u/-d), launches the GUI instead.\
";

/// Set on the re-exec'd child so it knows to actually launch the GUI instead
/// of detaching again (see `launch_gui`).
const GUI_CHILD_ENV: &str = "MUDL_GUI_CHILD";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(
        &args,
        &mut io::stdin(),
        &mut io::stdout(),
        &mut io::stderr(),
    )
}

fn run(
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> ExitCode {
    match parse(args) {
        Err(ArgError(message)) => {
            let _ = writeln!(stderr, "mudl: {message}");
            ExitCode::from(1)
        }
        Ok(ParsedArgs::Help) => {
            let _ = writeln!(stdout, "{HELP_TEXT}");
            ExitCode::SUCCESS
        }
        Ok(ParsedArgs::Version) => {
            let _ = writeln!(stdout, "mudl {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(ParsedArgs::InstallCli) => install_cli(stdout, stderr),
        Ok(ParsedArgs::LaunchGui(files)) => launch_gui(&files, stderr),
        Ok(ParsedArgs::Render(render_args)) => render(&render_args, stdin, stdout, stderr),
    }
}

/// Launches the GUI. The first time through, this process re-execs itself
/// detached from the controlling terminal (its own session, stdio dropped)
/// and exits immediately so the shell doesn't block on the GUI's lifetime —
/// mirroring how e.g. VS Code's `code` CLI hands off to its GUI process. The
/// re-exec'd child carries `GUI_CHILD_ENV` so it knows to actually launch the
/// GUI instead of detaching again.
fn launch_gui(files: &[String], stderr: &mut dyn Write) -> ExitCode {
    // Checked here (rather than left to the detached child) so this
    // error is still reported synchronously to the invoking shell.
    if files.is_empty() {
        let _ = writeln!(
            stderr,
            "mudl: no file given (folder index launch mode isn't implemented yet)"
        );
        return ExitCode::from(2);
    }

    if std::env::var_os(GUI_CHILD_ENV).is_none() {
        return match spawn_detached_gui(files) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                let _ = writeln!(stderr, "mudl: failed to detach GUI: {err}");
                ExitCode::from(2)
            }
        };
    }

    match mudl_gui::launch(files) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            let _ = writeln!(stderr, "mudl: {message}");
            ExitCode::from(2)
        }
    }
}

fn spawn_detached_gui(files: &[String]) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    unsafe {
        Command::new(exe)
            .args(files)
            .env(GUI_CHILD_ENV, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .pre_exec(|| {
                // Detach from the invoking shell's session so the GUI
                // survives the terminal closing (e.g. no SIGHUP on exit).
                libc::setsid();
                Ok(())
            })
            .spawn()?;
    }
    Ok(())
}

fn install_cli(stdout: &mut dyn Write, stderr: &mut dyn Write) -> ExitCode {
    let Some(home) = std::env::var_os("HOME") else {
        let _ = writeln!(stderr, "mudl: failed to install CLI: HOME is not set");
        return ExitCode::from(2);
    };
    match installer::install(&InstallerFileSystem, std::path::Path::new(&home)) {
        Ok(target) => {
            let _ = writeln!(stdout, "Installed mudl CLI at {}", target.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            let _ = writeln!(stderr, "mudl: failed to install CLI: {err}");
            ExitCode::from(2)
        }
    }
}

fn render(
    render_args: &RenderArgs,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> ExitCode {
    let options = RenderOptions {
        standalone: render_args.standalone,
        ..RenderOptions::default()
    };

    if render_args.files.is_empty() {
        let mut markdown = String::new();
        if let Err(err) = stdin.read_to_string(&mut markdown) {
            let _ = writeln!(stderr, "mudl: failed to read stdin: {err}");
            return ExitCode::from(2);
        }
        let base_dir = std::env::current_dir().unwrap_or_default();
        let html = render_one(&markdown, &base_dir, "", render_args, &options);
        let _ = writeln!(stdout, "{html}");
        return ExitCode::SUCCESS;
    }

    for file in &render_args.files {
        let path = std::path::Path::new(file);
        let markdown = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(err) => {
                let _ = writeln!(stderr, "mudl: failed to read {file}: {err}");
                return ExitCode::from(2);
            }
        };
        let base_dir = path.parent().unwrap_or(std::path::Path::new("."));
        let title = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let html = render_one(&markdown, base_dir, &title, render_args, &options);
        let _ = writeln!(stdout, "{html}");
    }

    ExitCode::SUCCESS
}

/// Renders one document's body, then — unless `--fragment` asked for the
/// bare body — wraps it into a complete, self-contained HTML document
/// (`docs/SECURITY.md` Finding 8, "CLI output is an unsanitized fragment":
/// the fragment shape itself was fine, since Finding 3's sanitizing already
/// covers it, but it meant the CLI's default output had no document wrapper
/// at all — matching upstream `mud`'s `--fragment` semantics, Appendix C of
/// `docs/IMPLEMENTATION-PLAN.md`, needed the wrapper to actually exist).
///
/// The full-document path always inlines local images as data URIs — same
/// as `--standalone` — because there's no server here to resolve a relative
/// `<img src>` against, so `--standalone` only has an independent effect in
/// `--fragment` mode.
fn render_one(
    markdown: &str,
    base_dir: &std::path::Path,
    title: &str,
    render_args: &RenderArgs,
    options: &RenderOptions,
) -> String {
    let body = match render_args.mode {
        Mode::Up => render_up(markdown, options),
        Mode::Down => render_down(markdown, options),
    };
    let body = if render_args.standalone || !render_args.fragment {
        rewrite_srcs_to_data_uris(&body, base_dir, &|p| std::fs::read(p))
    } else {
        body
    };

    if render_args.fragment {
        body
    } else {
        wrap_full_document(&body, title, render_args, options)
    }
}

/// Assembles a complete, self-contained HTML document around an already-
/// rendered (and, per `render_one`, already image-inlined) body: embedded
/// styles/scripts rather than `/assets/` references, since there's no
/// server here to serve them from.
fn wrap_full_document(
    body: &str,
    title: &str,
    render_args: &RenderArgs,
    options: &RenderOptions,
) -> String {
    let wrapper_class = match render_args.mode {
        Mode::Up => "up-mode-output",
        Mode::Down => "down-mode-output",
    };
    let wrapped_body = format!("<div class=\"{wrapper_class}\">{body}</div>");

    let selection = select_assets(&wrapped_body, options);

    let mode_css = match render_args.mode {
        Mode::Up => resources::MUD_UP_CSS,
        Mode::Down => resources::MUD_DOWN_CSS,
    };
    let mut styles = vec![resources::MUD_CSS.to_string(), mode_css.to_string()];
    let theme_name = render_args.theme.as_deref().unwrap_or("earthy");
    if let Some(theme_css) = resources::lookup(&format!("theme-{theme_name}.css")) {
        styles.push(theme_css.to_string());
    }
    for name in &selection.stylesheets {
        if let Some(css) = resources::lookup(name) {
            styles.push(css.to_string());
        }
    }

    // No server backs this file, so every script is inlined rather than
    // referenced by `src=` — there's no `/assets/` route to point at. This
    // is still safe under Finding 3's "no script from document content"
    // rule: these are our own fixed, bundled scripts, selected only by
    // which content markers are present, never markdown/HTML the document
    // supplied.
    let scripts: Vec<Script> = selection
        .scripts
        .iter()
        .filter_map(|name| resources::lookup(name))
        .map(|src| Script::Inline(src.to_string()))
        .collect();

    let mut html_classes = Vec::new();
    if render_args.line_numbers {
        html_classes.push("has-line-numbers".to_string());
    }
    if render_args.word_wrap {
        html_classes.push("has-word-wrap".to_string());
    }
    if render_args.readable_column {
        html_classes.push("is-readable-column".to_string());
    }

    let doc = HtmlDocument {
        title: title.to_string(),
        base_href: None,
        styles,
        // `'self' data:`, matching the GUI server's default
        // (`docs/SECURITY.md` Finding 4): remote images stay blocked, and
        // `data:` is what makes this file's own (already inlined) local
        // images visible.
        csp_img_src: vec!["'self'".to_string(), "data:".to_string()],
        csp_script_src: vec!["'unsafe-inline'".to_string()],
        html_classes,
        zoom_level: 1.0,
        body_content: wrapped_body,
        body_scripts: scripts,
    };
    doc.render()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    fn run_with(args: &[String], stdin: &str) -> (ExitCode, String, String) {
        let mut stdin_reader = stdin.as_bytes();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run(args, &mut stdin_reader, &mut stdout, &mut stderr);
        (
            code,
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
        )
    }

    #[test]
    fn help_exits_zero_and_prints_usage() {
        let (code, stdout, _) = run_with(&args(&["--help"]), "");
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(stdout.contains("USAGE"));
    }

    #[test]
    fn version_exits_zero_and_prints_version() {
        let (code, stdout, _) = run_with(&args(&["--version"]), "");
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn unknown_flag_exits_one() {
        let (code, _, stderr) = run_with(&args(&["--nonsense"]), "");
        assert_eq!(code, ExitCode::from(1));
        assert!(stderr.contains("--nonsense"));
    }

    #[test]
    fn no_flags_and_no_files_exits_two() {
        let (code, _, stderr) = run_with(&args(&[]), "");
        assert_eq!(code, ExitCode::from(2));
        assert!(stderr.contains("no file given"));
    }

    #[test]
    fn missing_file_exits_two() {
        let (code, _, stderr) = run_with(&args(&["-u", "/no/such/file.md"]), "");
        assert_eq!(code, ExitCode::from(2));
        assert!(stderr.contains("/no/such/file.md"));
    }

    #[test]
    fn stdin_is_rendered_when_no_files_given() {
        let (code, stdout, _) = run_with(&args(&["-u"]), "# Hello");
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(stdout.contains("<h1"));
        assert!(stdout.contains("Hello"));
    }

    #[test]
    fn down_mode_renders_raw_source() {
        let (code, stdout, _) = run_with(&args(&["-d"]), "line one");
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(stdout.contains("line one"));
    }

    #[test]
    fn default_output_is_a_complete_html_document() {
        let (code, stdout, _) = run_with(&args(&["-u"]), "# Hello");
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(stdout.starts_with("<!DOCTYPE html>"));
        assert!(stdout.contains("<html"));
        assert!(stdout.contains("Content-Security-Policy"));
        assert!(stdout.contains("<h1"));
        assert!(stdout.trim_end().ends_with("</html>"));
    }

    #[test]
    fn fragment_flag_emits_bare_body_with_no_wrapper() {
        let (code, stdout, _) = run_with(&args(&["-u", "--fragment"]), "# Hello");
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(!stdout.contains("<!DOCTYPE"));
        assert!(!stdout.contains("<html"));
        assert!(stdout.contains("<h1"));
    }

    #[test]
    fn fragment_short_flag_is_equivalent() {
        let (code, stdout, _) = run_with(&args(&["-u", "-f"]), "# Hello");
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(!stdout.contains("<!DOCTYPE"));
    }

    #[test]
    fn default_output_embeds_the_selected_theme() {
        let (code, stdout, _) = run_with(&args(&["-u", "--theme", "riot"]), "# Hello");
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(stdout.contains("Theme: Riot"));
    }

    #[test]
    fn default_output_applies_line_numbers_and_word_wrap_classes() {
        let (code, stdout, _) =
            run_with(&args(&["-d", "--line-numbers", "--word-wrap"]), "line one");
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(stdout.contains("has-line-numbers"));
        assert!(stdout.contains("has-word-wrap"));
    }
}
