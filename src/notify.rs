//! Fallback for desktops where no overlay window is possible (GNOME on Wayland has no
//! layer-shell): each card becomes a freedesktop desktop notification. There is no Clawd and the
//! desktop owns timing and placement. Uses `notify-send` or `gdbus` rather than a D-Bus library,
//! so the binary stays small.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::config::Config;
use crate::core::Card;
use crate::log;
use std::process::{Command, Stdio};

/// Shows each card from stdin as a notification, until the daemon is done with us.
pub fn run() {
    let cfg = Config::load();
    let timeout_ms = (cfg.duration_secs * 1000.0).round() as u32;
    crate::overlay::serve_cards(|card| {
        let ok = commands(&card, timeout_ms).into_iter().any(|(prog, args)| {
            Command::new(prog)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        });
        if !ok {
            log::line("notify: neither notify-send nor gdbus could deliver the notification");
        }
    });
}

/// Commands to try in order for `card`. A new card for the same session replaces the previous
/// notification where the daemon supports a sync hint (GNOME, KDE, dunst).
fn commands(card: &Card, timeout_ms: u32) -> Vec<(&'static str, Vec<String>)> {
    let tag = format!("clawdtoast-{}", card.session_id);
    let title = card.title();
    let notify_send = vec![
        "--app-name=clawdtoast".to_string(),
        format!("--expire-time={timeout_ms}"),
        format!("--hint=string:x-canonical-private-synchronous:{tag}"),
        format!("--hint=string:x-dunst-stack-tag:{tag}"),
        "--".to_string(),
        title.clone(),
        card.body.clone(),
    ];
    let gdbus = vec![
        "call".into(),
        "--session".into(),
        "--dest=org.freedesktop.Notifications".into(),
        "--object-path=/org/freedesktop/Notifications".into(),
        "--method=org.freedesktop.Notifications.Notify".into(),
        "clawdtoast".into(),
        "0".into(),
        "''".into(),
        gvariant_str(&title),
        gvariant_str(&card.body),
        "[]".into(),
        format!("{{'x-canonical-private-synchronous': <{}>}}", gvariant_str(&tag)),
        timeout_ms.to_string(),
    ];
    vec![("notify-send", notify_send), ("gdbus", gdbus)]
}

/// A GVariant text-format string literal (gdbus parses its arguments as GVariant text).
fn gvariant_str(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Kind;

    fn card(body: &str) -> Card {
        Card {
            session_id: "abcd1234".into(),
            kind: Kind::Done,
            project: "proj".into(),
            body: body.into(),
            badge: None,
            ts: 0,
            pid: 0,
        }
    }

    #[test]
    fn gvariant_escapes_quotes_backslashes_and_newlines() {
        assert_eq!(gvariant_str(r"it's a \ test"), r"'it\'s a \\ test'");
        assert_eq!(gvariant_str("a\nb"), r"'a\nb'");
    }

    #[test]
    fn notify_send_gets_title_body_timeout_and_a_replace_hint() {
        let cmds = commands(&card("Fixed it."), 5000);
        let (prog, args) = &cmds[0];
        assert_eq!(*prog, "notify-send");
        assert!(args.contains(&"--expire-time=5000".to_string()));
        assert!(args.iter().any(|a| a == "--hint=string:x-canonical-private-synchronous:clawdtoast-abcd1234"));
        // Title and body come last, after `--`, so text starting with a dash is not an option.
        assert_eq!(&args[args.len() - 3..], ["--", "Done · proj", "Fixed it."]);
    }

    #[test]
    fn gdbus_args_are_quoted_for_the_variant_parser() {
        let cmds = commands(&card("it's done"), 3000);
        let (prog, args) = &cmds[1];
        assert_eq!(*prog, "gdbus");
        assert!(args.contains(&r"'it\'s done'".to_string()));
        assert_eq!(args.last().unwrap(), "3000");
    }
}
