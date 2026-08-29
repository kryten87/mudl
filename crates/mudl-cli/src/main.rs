mod args;

use std::io::{self, Read, Write};
use std::process::ExitCode;

use args::{parse, ArgError, Mode, ParsedArgs, RenderArgs};
use mudl_core::images::rewrite_srcs_to_data_uris;
use mudl_core::options::RenderOptions;
use mudl_core::render::{render_down, render_up};

const HELP_TEXT: &str = "\
mudl - A Perfect Markdown Viewer

USAGE:
    mudl [FLAGS] [FILE...]

FLAGS:
    -h, --help              Print this help message and exit
    -v, --version           Print the version and exit
    -u, --html-up           Render Up-mode (styled) HTML to stdout
    -d, --html-down         Render Down-mode (raw source) HTML to stdout
        --standalone        Inline local images as data URIs
    -f, --fragment          Emit body-only HTML, no document wrapper
        --line-numbers      Show line numbers (Down mode)
        --word-wrap         Wrap long lines (Down mode)
        --readable-column   Constrain body width to a readable column
        --theme NAME        One of: austere, blues, earthy, riot, system

With no FILE arguments, reads Markdown from stdin. With no render flag
(-u/-d), launches the GUI instead.\
";

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
        Ok(ParsedArgs::LaunchGui(files)) => match mudl_gui::launch(&files) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                let _ = writeln!(stderr, "mudl: {message}");
                ExitCode::from(2)
            }
        },
        Ok(ParsedArgs::Render(render_args)) => render(&render_args, stdin, stdout, stderr),
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
        let html = render_one(&markdown, &base_dir, render_args, &options);
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
        let html = render_one(&markdown, base_dir, render_args, &options);
        let _ = writeln!(stdout, "{html}");
    }

    ExitCode::SUCCESS
}

fn render_one(
    markdown: &str,
    base_dir: &std::path::Path,
    render_args: &RenderArgs,
    options: &RenderOptions,
) -> String {
    let body = match render_args.mode {
        Mode::Up => render_up(markdown, options),
        Mode::Down => render_down(markdown, options),
    };
    if render_args.standalone {
        rewrite_srcs_to_data_uris(&body, base_dir, &|p| std::fs::read(p))
    } else {
        body
    }
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
}
