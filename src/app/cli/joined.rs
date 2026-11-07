use std::ffi::OsString;

use clap::CommandFactory;

use super::Cli;
use crate::base::failure::Failure;

pub struct Joined {
    pub command: String,
    pub failure: Failure,
}

fn names_flag(cmd: &clap::Command, word: &str) -> bool {
    cmd.get_arguments().any(|a| {
        a.get_long().is_some_and(|l| word == format!("--{l}"))
            || a.get_short().is_some_and(|s| word == format!("-{s}"))
    })
}

pub fn joined(args: &[OsString]) -> Option<Joined> {
    let cli = Cli::command();
    let command = args.get(1)?.to_str()?.trim();
    let first = command.split_whitespace().next()?;
    let sub = cli.find_subcommand(first)?;
    let together = args.iter().skip(1).filter_map(|a| a.to_str()).find(|a| {
        let mut words = a.split_whitespace();
        match (words.next(), words.next()) {
            (Some(w), Some(_)) => (*a == command && w == first) || names_flag(sub, w),
            _ => false,
        }
    })?;
    let failure = Failure::usage(format!("burrow got \"{together}\" as one argument"))
        .hint(format!(
            "give each argument on its own line: in an AI app's Arguments field (like Cherry Studio's), write {} on separate lines; or paste the JSON from: {}",
            together
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(", "),
            crate::base::failure::cmd("mcp --print-config")
        ));
    Some(Joined {
        command: first.to_string(),
        failure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(args: &[&str]) -> Option<Joined> {
        let mut all = vec![OsString::from("burrow")];
        all.extend(args.iter().map(OsString::from));
        joined(&all)
    }

    #[test]
    fn a_command_or_a_flag_with_its_value_in_one_argument_is_named() {
        let j = of(&["mcp -c /Users/me/visitor.yaml"]).unwrap();
        assert_eq!(j.command, "mcp");
        assert_eq!(
            j.failure.message,
            "burrow got \"mcp -c /Users/me/visitor.yaml\" as one argument"
        );
        assert_eq!(j.failure.exit, crate::base::failure::EXIT_USAGE);
        let hint = j.failure.hint.unwrap();
        assert!(hint.contains("its own line"), "{hint}");
        assert!(hint.contains("mcp, -c, /Users/me/visitor.yaml"), "{hint}");
        assert!(of(&["mcp", "-c /Users/me/visitor.yaml"]).is_some());
        assert!(of(&["mcp", "-c", "/x/v.yaml", "--read-only --http"]).is_some());
        assert!(of(&["validate", "--config my.yaml"]).is_some());
    }

    #[test]
    fn values_with_spaces_are_left_alone() {
        assert!(of(&["mcp", "-c", "/Users/me/my files/visitor.yaml"]).is_none());
        assert!(of(&["init", "visitor", "--name", "a b"]).is_none());
        assert!(of(&["mcp"]).is_none());
        assert!(of(&["nonsense words"]).is_none());
        assert!(of(&[]).is_none());
    }
}
