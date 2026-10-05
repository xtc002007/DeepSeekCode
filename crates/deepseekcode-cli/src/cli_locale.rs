//! CLI language selection and runtime help localization.
//!
//! `clap` generates stable command structure from `Cli`, but derive attributes
//! can only carry fixed English help text. After the process locale is known
//! this module rewrites the generated `Command` in place, so `--help` uses the
//! message catalog while command names, flags and parsing behaviour stay
//! exactly as derived.

use clap::Command;
use deepseekcode_i18n::{LanguagePref, Locale};

#[cfg(test)]
use super::Cli;
#[cfg(test)]
use clap::CommandFactory;

/// Raw process arguments after the program name, lossily decoded.
///
/// Directory and language tags are filesystem/command-line text; non-UTF-8
/// bytes cannot be a locale tag or a useful config hint, so lossy decoding is
/// the correct conservative choice here.
pub fn raw_args_after_program() -> Vec<String> {
    std::env::args().skip(1).collect()
}

/// Explicit `--lang <tag>` / `--lang=<tag>` override, if present.
pub fn lang_override(args: &[String]) -> Option<LanguagePref> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        if let Some(tag) = arg.strip_prefix("--lang=") {
            return Some(LanguagePref::parse(tag));
        }
        if arg == "--lang" {
            if let Some(tag) = iter.next() {
                return Some(LanguagePref::parse(tag));
            }
            return Some(LanguagePref::Auto);
        }
    }
    None
}

/// Best-effort `--directory` / `-d` hint for config lookup.
///
/// Help can be requested before any command runs, so the selected project
/// directory is recovered from the raw arguments without invoking `clap`.
pub fn directory_hint_from_raw(args: &[String]) -> String {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        if let Some(value) = arg.strip_prefix("--directory=") {
            return value.to_string();
        }
        if arg == "--directory" || arg == "-d" {
            if let Some(value) = iter.next() {
                return value.clone();
            }
            return ".".to_string();
        }
        if let Some(value) = arg.strip_prefix("-d") {
            if !value.is_empty() && !value.starts_with('-') {
                return value.to_string();
            }
        }
    }
    ".".to_string()
}

/// Resolve the locale preference without touching global state.
///
/// Precedence: explicit `--lang`, then a supplied project-config value, then
/// detection. Production passes `config.ui.language`; tests pass fixtures.
pub fn preference_for_raw(args: &[String], config_language: Option<&str>) -> LanguagePref {
    if let Some(explicit) = lang_override(args) {
        return explicit;
    }
    if let Some(value) = config_language {
        return LanguagePref::parse(value);
    }
    LanguagePref::Auto
}

/// Resolve and install the CLI locale.
///
/// This runs before `clap` parses, so both translated `--help` and translated
/// command output observe the same language.
pub fn init_cli_locale(args: &[String]) -> Locale {
    let directory = directory_hint_from_raw(args);
    let config_language =
        deepseekcode_config::load_effective_config(camino::Utf8Path::new(&directory))
            .ok()
            .and_then(|config| config.ui.language);
    deepseekcode_i18n::init_from_preference(preference_for_raw(args, config_language.as_deref()))
}

/// Rewrite a derived `clap::Command` with catalog help text.
pub fn localize_command(cmd: &mut Command) {
    let mut path = Vec::new();
    let localized = localize_command_at(std::mem::replace(cmd, Command::new("")), &mut path);
    *cmd = localized;
}

fn localize_command_at(mut cmd: Command, path: &mut Vec<String>) -> Command {
    if let Some(key) = command_help_key(path) {
        let text = deepseekcode_i18n::t(key);
        cmd = cmd.about(text.clone()).long_about(text);
    }
    let ids: Vec<String> = cmd
        .get_arguments()
        .map(|arg| arg.get_id().as_str().to_string())
        .collect();
    for id in ids {
        if let Some(key) = arg_help_key(&id) {
            let text = deepseekcode_i18n::t(key);
            cmd = cmd.mut_arg(&id, |arg| arg.help(text.clone()).long_help(text.clone()));
        }
    }
    let names: Vec<String> = cmd
        .get_subcommands()
        .map(|subcommand| subcommand.get_name().to_string())
        .collect();
    for name in names {
        path.push(name.clone());
        cmd = cmd.mut_subcommand(&name, |subcommand| localize_command_at(subcommand, path));
        path.pop();
    }
    cmd
}

/// Catalog key for a command path, e.g. `["session", "fork"]`.
fn command_help_key(path: &[String]) -> Option<&'static str> {
    let joined = path.join("/");
    Some(match joined.as_str() {
        "" => "cli.about",
        "run" => "cli.cmd.run",
        "serve" => "cli.cmd.serve",
        "upgrade" => "cli.cmd.upgrade",
        "session-list" => "cli.cmd.session_list",
        "diagnostics" => "cli.cmd.diagnostics",
        "agent" => "cli.cmd.agent",
        "model" => "cli.cmd.model",
        "provider" => "cli.cmd.provider",
        "prompt" => "cli.cmd.prompt",
        "session" => "cli.cmd.session",
        "mcp" => "cli.cmd.mcp",
        "fs" => "cli.cmd.fs",
        "skills" => "cli.cmd.skills",
        "plugins" => "cli.cmd.plugins",
        "worktree" => "cli.cmd.worktree",
        "project" => "cli.cmd.project",
        "config" => "cli.cmd.config",
        "health" => "cli.cmd.health",
        "policy" => "cli.cmd.policy",
        "diagnostics/export" => "cli.sub.diagnostics_export",
        "agent/list" => "cli.sub.agent_list",
        "agent/set" => "cli.sub.agent_set",
        "model/list" => "cli.sub.model_list",
        "model/set" => "cli.sub.model_set",
        "provider/list" => "cli.sub.provider_list",
        "provider/connect" => "cli.sub.provider_connect",
        "provider/disconnect" => "cli.sub.provider_disconnect",
        "session/list" => "cli.sub.session_list",
        "session/fork" => "cli.sub.session_fork",
        "session/export" => "cli.sub.session_export",
        "session/revert" => "cli.sub.session_revert",
        "session/compact" => "cli.sub.session_compact",
        "session/move" => "cli.sub.session_move",
        "session/interrupt" => "cli.sub.session_interrupt",
        "session/delete" => "cli.sub.session_delete",
        "session/todo" => "cli.sub.session_todo",
        "session/plans" => "cli.sub.session_plans",
        "session/answer" => "cli.sub.session_answer",
        "session/revert-commit" => "cli.sub.session_revert_commit",
        "session/revert-clear" => "cli.sub.session_revert_clear",
        "mcp/list" => "cli.sub.mcp_list",
        "mcp/connect" => "cli.sub.mcp_connect",
        "mcp/disconnect" => "cli.sub.mcp_disconnect",
        "fs/tree" => "cli.sub.fs_tree",
        "fs/file" => "cli.sub.fs_file",
        "worktree/list" => "cli.sub.worktree_list",
        "worktree/create" => "cli.sub.worktree_create",
        "worktree/remove" => "cli.sub.worktree_remove",
        "worktree/prune" => "cli.sub.worktree_prune",
        "project/directories" => "cli.sub.project_directories",
        "project/directories/list" => "cli.sub.project_directories_list",
        "project/directories/add" => "cli.sub.project_directories_add",
        "policy/pending" => "cli.sub.policy_pending",
        "policy/reply" => "cli.sub.policy_reply",
        "policy/grants" => "cli.sub.policy_grants",
        "policy/grant-delete" => "cli.sub.policy_grant_delete",
        _ => return None,
    })
}

/// Catalog key for a clap argument id.
fn arg_help_key(id: &str) -> Option<&'static str> {
    Some(match id {
        "url" => "cli.arg.url",
        "password" => "cli.arg.password",
        "lang" => "cli.arg.lang",
        "directory" => "cli.arg.directory",
        "auto" => "cli.arg.auto",
        "port" => "cli.arg.port",
        "host" => "cli.arg.host",
        "check_only" => "cli.arg.check_only",
        "output" => "cli.arg.output",
        "run_id" => "cli.arg.run_id",
        "agent" => "cli.arg.agent",
        "model" => "cli.arg.model",
        "provider" => "cli.arg.provider",
        "api_key" => "cli.arg.api_key",
        "base_url" => "cli.arg.base_url",
        "session" => "cli.arg.session",
        "until" => "cli.arg.until",
        "no_sanitize" => "cli.arg.no_sanitize",
        "message" => "cli.arg.message",
        "offset" => "cli.arg.offset",
        "limit" => "cli.arg.limit",
        "path" => "cli.arg.path",
        "branch" => "cli.arg.branch",
        "name" => "cli.arg.name",
        "id" => "cli.arg.id",
        "reply" => "cli.arg.reply",
        "text" => "cli.arg.text",
        "delivery" => "cli.arg.delivery",
        "resume" => "cli.arg.resume",
        "target" => "cli.arg.target",
        _ => return None,
    })
}

#[cfg(test)]
fn unmapped_commands() -> Vec<String> {
    let mut missing = Vec::new();
    let mut path = Vec::new();
    collect_unmapped_commands(&Cli::command(), &mut path, &mut missing);
    missing
}

#[cfg(test)]
fn collect_unmapped_commands(cmd: &Command, path: &mut Vec<String>, missing: &mut Vec<String>) {
    if command_help_key(path).is_none() {
        missing.push(if path.is_empty() {
            "<root>".to_string()
        } else {
            path.join("/")
        });
    }
    for subcommand in cmd.get_subcommands() {
        path.push(subcommand.get_name().to_string());
        collect_unmapped_commands(subcommand, path, missing);
        path.pop();
    }
}

#[cfg(test)]
fn unmapped_args() -> Vec<String> {
    let mut missing = Vec::new();
    let mut path = Vec::new();
    collect_unmapped_args(&Cli::command(), &mut path, &mut missing);
    missing.sort();
    missing.dedup();
    missing
}

#[cfg(test)]
fn collect_unmapped_args(cmd: &Command, path: &mut Vec<String>, missing: &mut Vec<String>) {
    for arg in cmd.get_arguments() {
        let id = arg.get_id().as_str().to_string();
        if arg_help_key(&id).is_none() {
            missing.push(format!("{}/{}", path.join("/"), id));
        }
    }
    for subcommand in cmd.get_subcommands() {
        path.push(subcommand.get_name().to_string());
        collect_unmapped_args(subcommand, path, missing);
        path.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_a_catalog_key() {
        assert!(
            unmapped_commands().is_empty(),
            "unmapped clap commands: {:?}",
            unmapped_commands()
        );
    }

    #[test]
    fn every_localized_command_key_resolves() {
        for key in [
            "cli.about",
            "cli.cmd.run",
            "cli.cmd.serve",
            "cli.cmd.upgrade",
            "cli.cmd.session_list",
            "cli.cmd.diagnostics",
            "cli.cmd.agent",
            "cli.cmd.model",
            "cli.cmd.provider",
            "cli.cmd.prompt",
            "cli.cmd.session",
            "cli.cmd.mcp",
            "cli.cmd.fs",
            "cli.cmd.skills",
            "cli.cmd.plugins",
            "cli.cmd.worktree",
            "cli.cmd.project",
            "cli.cmd.config",
            "cli.cmd.health",
            "cli.cmd.policy",
            "cli.sub.session_revert_commit",
            "cli.sub.session_revert_clear",
            "cli.sub.policy_grant_delete",
        ] {
            assert!(
                deepseekcode_i18n::catalog::lookup(deepseekcode_i18n::Locale::En, key).is_some(),
                "{key} is missing"
            );
        }
    }

    #[test]
    fn only_freeform_positionals_lack_argument_help() {
        // `run/prompt` is the command's free-form input. Every named option
        // has a catalog entry; `text` positionals reuse `cli.arg.text`.
        assert_eq!(unmapped_args(), vec!["run/prompt".to_string()]);
    }

    #[test]
    fn localized_help_uses_catalog_text() {
        // The process locale is English by default in tests, so this checks
        // the wiring rather than a translation: every generated help string
        // must come from the catalog instead of the derive fallback.
        let mut cmd = Cli::command();
        localize_command(&mut cmd);
        let about = cmd
            .get_about()
            .map(ToString::to_string)
            .expect("root about");
        assert_eq!(about, "DeepSeekCode command-line interface");

        let run = cmd.find_subcommand("run").expect("run subcommand");
        assert_eq!(
            run.get_about().map(ToString::to_string).as_deref(),
            Some("Run a single prompt non-interactively")
        );
        let directory = run
            .get_arguments()
            .find(|arg| arg.get_id() == "directory")
            .expect("directory option");
        assert_eq!(
            directory.get_help().map(ToString::to_string).as_deref(),
            Some("Project directory")
        );
        let lang = cmd
            .get_arguments()
            .find(|arg| arg.get_id() == "lang")
            .expect("lang option");
        assert_eq!(
            lang.get_help().map(ToString::to_string).as_deref(),
            Some("Interface language tag (auto, en, zh-Hans, …)")
        );
    }

    #[test]
    fn lang_and_directory_hints_are_recovered_before_parsing() {
        assert_eq!(
            lang_override(&[
                "session".to_string(),
                "list".to_string(),
                "--lang=ja".to_string()
            ]),
            Some(LanguagePref::Locale(Locale::Ja))
        );
        assert_eq!(
            lang_override(&["--lang".to_string(), "pt-BR".to_string()]),
            Some(LanguagePref::Locale(Locale::PtBr))
        );
        assert_eq!(
            lang_override(&["--".to_string(), "--lang=ja".to_string()]),
            None
        );
        assert_eq!(
            directory_hint_from_raw(&[
                "-d".to_string(),
                "/tmp/project".to_string(),
                "session".to_string()
            ]),
            "/tmp/project"
        );
        assert_eq!(
            directory_hint_from_raw(&["--directory=/tmp/other".to_string()]),
            "/tmp/other"
        );
        assert_eq!(
            directory_hint_from_raw(&["session".to_string(), "list".to_string()]),
            "."
        );
    }

    #[test]
    fn explicit_lang_beats_config_and_config_beats_detection() {
        assert_eq!(
            preference_for_raw(&["--lang=ru".to_string()], Some("ja")),
            LanguagePref::Locale(Locale::Ru)
        );
        assert_eq!(
            preference_for_raw(&[], Some("zh-Hant")),
            LanguagePref::Locale(Locale::ZhHant)
        );
        assert_eq!(preference_for_raw(&[], Some("klingon")), LanguagePref::Auto);
        assert_eq!(preference_for_raw(&[], None), LanguagePref::Auto);
    }
}
